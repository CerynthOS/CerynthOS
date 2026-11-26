use anyhow::Result;
use cerynth_agent_api::ResourceBudget;
use cerynth_resources::{
    admission::{Capacity, Decision},
    authority::{AcceleratorPolicy, Authority, Config},
    global::Demand,
    pressure::{self, Sample},
};
use std::collections::BTreeMap;
use uuid::Uuid;

fn healthy() -> Sample {
    Sample {
        version: 1,
        observed_at_ms: pressure::now_ms(),
        available_memory_bytes: Some(1024_u64.pow(3)),
        memory_some_avg10: Some(0.0),
        memory_full_avg10: Some(0.0),
        cpu_some_avg10: Some(0.0),
        io_full_avg10: Some(0.0),
    }
}
fn config() -> Config {
    Config {
        version: 1,
        capacity: Capacity {
            memory_bytes: 768 * 1024 * 1024,
            cpu_quota_micros_per_second: 2_000_000,
        },
        inference_limit: 2,
        accelerators: BTreeMap::from([(
            "fixture-gpu".into(),
            AcceleratorPolicy {
                total_bytes: 1024 * 1024 * 1024,
                desktop_reserve_bytes: 256 * 1024 * 1024,
            },
        )]),
    }
}
fn demand() -> Demand {
    Demand {
        schema_version: 1,
        request_id: Uuid::new_v4(),
        session_id: Uuid::new_v4(),
        parent_session_id: None,
        project: "public-fixture".into(),
        requested: ResourceBudget {
            cpu_quota_micros_per_second: Some(1_000_000),
            memory_max_bytes: 512 * 1024 * 1024,
            memory_high_bytes: 128 * 1024 * 1024,
            inference_concurrency: 1,
            gpu_memory_bytes: 512 * 1024 * 1024,
            ..ResourceBudget::default()
        },
        minimum: None,
        gpu_device_id: Some("fixture-gpu".into()),
        inference_endpoint_id: Some("local".into()),
    }
}
#[test]
fn independent_members_share_durable_atomic_limits_and_cannot_reap_each_other() -> Result<()> {
    let root = tempfile::tempdir()?;
    let directory = root.path().join("authority");
    Authority::initialize(&directory, config())?;
    let first = Authority::open(&directory)?;
    let second = Authority::open(&directory)?;
    let a = demand();
    let mut b = demand();
    let free = BTreeMap::from([("fixture-gpu".into(), 1024 * 1024 * 1024)]);
    let receipt = first.reserve("member-a", &a, &healthy(), &free)?;
    assert!(matches!(receipt.decision, Decision::Accept { .. }) && !receipt.kernel_applied);
    assert!(first.reserve("member-a", &a, &healthy(), &free)?.existing);
    assert!(matches!(
        second.reserve("member-b", &b, &healthy(), &free)?.decision,
        Decision::Queue { .. }
    ));
    b.minimum = Some(ResourceBudget {
        memory_max_bytes: 256 * 1024 * 1024,
        memory_high_bytes: 128 * 1024 * 1024,
        gpu_memory_bytes: 256 * 1024 * 1024,
        ..b.requested.clone()
    });
    assert!(matches!(
        second.reserve("member-b", &b, &healthy(), &free)?.decision,
        Decision::Reduce { .. }
    ));
    assert_eq!(second.view()?.claims.len(), 2);
    assert!(second.release("member-b", a.session_id).is_err());
    drop(first);
    drop(second);
    let recovered = Authority::open(&directory)?;
    assert_eq!(
        recovered.view()?.claims.len(),
        2,
        "process disappearance must not release active subtrees"
    );
    let mut changed = a.clone();
    changed.requested.timeout_seconds += 1;
    assert!(
        recovered
            .reserve("member-a", &changed, &healthy(), &free)
            .is_err()
    );
    assert!(
        recovered
            .reserve("member-b", &a, &healthy(), &free)
            .is_err()
    );
    let mut stale = healthy();
    stale.observed_at_ms = 0;
    assert!(
        recovered
            .reserve("member-c", &demand(), &stale, &free)
            .is_err()
    );
    recovered.release("member-a", a.session_id)?;
    recovered.release("member-b", b.session_id)?;
    assert!(recovered.view()?.claims.is_empty());
    assert!(Authority::initialize(&directory, config()).is_err());
    Ok(())
}
#[test]
fn concurrent_process_style_clients_never_oversubscribe() -> Result<()> {
    let root = tempfile::tempdir()?;
    let directory = root.path().join("authority");
    Authority::initialize(&directory, config())?;
    let clients = (0..12)
        .map(|_| Authority::open(&directory))
        .collect::<Result<Vec<_>>>()?;
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(clients.len()));
    let threads: Vec<_> = clients
        .into_iter()
        .enumerate()
        .map(|(i, client)| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let request = demand();
                barrier.wait();
                for _ in 0..200 {
                    match client.reserve(
                        &format!("member-{i}"),
                        &request,
                        &healthy(),
                        &BTreeMap::from([("fixture-gpu".into(), 1024 * 1024 * 1024)]),
                    ) {
                        Ok(receipt) => {
                            return Ok(matches!(receipt.decision, Decision::Accept { .. }));
                        }
                        Err(error) if error.to_string().contains("transaction busy") => {
                            std::thread::sleep(std::time::Duration::from_millis(2));
                        }
                        Err(error) => return Err(error),
                    }
                }
                anyhow::bail!("authority transaction did not converge")
            })
        })
        .collect();
    let accepted = threads
        .into_iter()
        .map(|t| t.join().expect("fixture worker"))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .filter(|b| *b)
        .count();
    assert_eq!(accepted, 1);
    assert_eq!(Authority::open(&directory)?.view()?.claims.len(), 1);
    Ok(())
}
