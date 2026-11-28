# Completing physical and public-release acceptance

The tools do not manufacture approvals, select a disposable machine, install
onto the development workstation, reboot it, or switch its scheduler. An
administrator supplies the authorized target and actual review decisions.

## Physical checkpoints

```sh
python3 tools/platform_hardware_probe.py --output hardware.json
```

The probe records CPU/RAM, PCI driver binding, connected displays, kernel and
virtualization. It excludes serials, machine identity and credentials. Observing
another Linux installation does not qualify the Cerynth candidate.

On the selected physical target, install the exact authenticated candidate and
keep the serial-bound installer's root-owned private journal. After booting
that image, run as its administrator:

```sh
python3 tools/platform_physical_recovery.py prepare \
  --directory /var/lib/cerynth/physical-run \
  --image-sha256 IMAGE_SHA256 --expected-overlay-sha256 OVERLAY_MANIFEST_SHA256 \
  --install-journal /root/physical-install.json
```

Use the actual digests. The overlay identity is the SHA256 of the selected
build's `/usr/share/doc/cerynth/image-overlay.json`. Prepare verifies the
deployment context and installation readback record, then fsyncs a random
marker and records a hash of the boot ID. A VM or unrelated installed image is
rejected before creating checkpoint state.

After the authorized normal reboot, run the same command with phase
`reboot-check`, omitting `--install-journal`. After the separately authorized
physical power-cut procedure, run phase `power-cut-check` with
`--operator-confirmed-power-cut`. Each requires a distinct boot and unchanged
marker. No checkpoint command initiates a reboot or power cut.

Software cannot distinguish a physical cut from a normal shutdown. The flag
records the operator's explicit attestation alongside observed marker/boot/image
details. These checkpoints cover recovery; rendering, driver correctness and
workload responsiveness still need their actual observations and review.

## Exact-candidate review record

```sh
python3 tools/platform_release_acceptance.py template \
  --bundle /absolute/candidate-bundle --bundle-public-key /absolute/build-publisher.pub \
  --release-signer-public-key /absolute/release-publisher.pub \
  --source-revision SOURCE_COMMIT --contract-id CONTRACT_FINGERPRINT \
  --output /absolute/acceptance.pending.json
```

Use the actual source commit and generated API fingerprint for that build.
For rootfs/UKI-only updates, also identify the tested complete image using
`--qualified-image-sha256`. Rebuild from reviewed source and verify deployment
configuration before requesting public acceptance; an old development artifact
is not automatically eligible.

Every template approval starts false. Responsible maintainers record the real
Person 1/2/4 interface decisions, dependency/license review and release decision,
with reviewer identities and evidence references. Bind the completed physical
report's SHA256. The candidate binds source commit, contract, version, sequence,
all product/inventory hashes, qualified image and selected publisher identity.

The configured authority signs the completed record outside the checker:

```sh
openssl pkeyutl -sign -inkey /secure/review-authority.key -rawin \
  -in /absolute/acceptance.json -out /absolute/acceptance.sig
```

This is a maintainer action after actual review, not an instruction to invent
decisions or discover credentials. The checker never accepts that private key.
Select the authority public key independently of the bundle. With separate
authority and publisher keys, the publisher alone cannot forge review acceptance.

## Public signing and consumption

Signing remains development-only by default. Public signing requires the
externally signed acceptance and matching physical report:

```sh
python3 tools/release_os_image.py sign --channel release \
  --image /absolute/candidate.raw --inventory /absolute/candidate-sbom.json \
  --private-key /secure/release-publisher.key --output /absolute/new-release \
  --version RELEASE_VERSION --sequence RELEASE_SEQUENCE \
  --source-revision SOURCE_COMMIT --contract-id CONTRACT_FINGERPRINT \
  --acceptance /absolute/acceptance.json --acceptance-signature /absolute/acceptance.sig \
  --acceptance-authority-public-key /absolute/review-authority.pub \
  --physical-recovery /absolute/recovery.json
```

The reviewed record must name exactly those payload files, version and sequence.
The resulting bundle carries the acceptance record/signature and physical
report, all bound by the publisher manifest. Secure Boot remains separate.

```sh
python3 tools/release_os_image.py verify --require-release \
  --directory /absolute/release --public-key /absolute/release-publisher.pub \
  --acceptance-authority-public-key /absolute/review-authority.pub
```

Use the same `--require-release` and authority-key options for public installation
and `cerynth-slot-update stage`. Verification checks both signatures against the
externally selected keys. Required public mode rejects signed development
artifacts. Previously built development images must be rebuilt to include the
updated tools; a signed sidecar does not retroactively update their verifier.

Tests use synthetic records and disposable keys only. A positive fixture proves
the implementation; it supplies no real team approval or physical acceptance.
The real gates remain explicit in [platform status](person1-status.md).
