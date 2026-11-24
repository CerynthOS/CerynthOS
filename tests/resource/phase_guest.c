/* Synthetic phase slice experiment, only as init in an isolated two-CPU VM. */
#define main resource_suite_main
static void finish(void) __attribute__((noreturn));
#include "platform_guest.c"
#undef main
#include <sched.h>

static uint64_t field(pid_t pid, unsigned wanted) {
    char file[80]; snprintf(file, sizeof(file), "/proc/%d/stat", pid);
    char *text = get(file); if (!text) return 0;
    text = strrchr(text, ')'); if (!text) return 0;
    text += 2; unsigned index = 3;
    char *save = NULL;
    for (char *token = strtok_r(text, " ", &save); token; token = strtok_r(NULL, " ", &save), index++)
        if (index == wanted) return strtoull(token, NULL, 10);
    return 0;
}

static pid_t phase_scheduler(const char *binary, int phase) {
    pid_t child = fork();
    if (!child) {
        if (phase) setenv("CERYNTH_SCX_PHASE_SNAPSHOT", "/run/cerynth/phase.json", 1);
        execl(binary, binary, "--profile", "balanced", NULL); perror("exec phase scheduler"); _exit(127);
    }
    return child;
}

int main(void) {
    if (getpid() != 1) { fprintf(stderr, "VM init only\n"); return 2; }
    setvbuf(stdout, NULL, _IOLBF, 0);
    mount("devtmpfs", "/dev", "devtmpfs", 0, NULL);
    mount("proc", "/proc", "proc", 0, NULL);
    mount("sysfs", "/sys", "sysfs", 0, NULL);
    mkdir("/sys/fs/cgroup", 0755); mount("none", "/sys/fs/cgroup", "cgroup2", 0, NULL);
    mkdir("/run/cerynth", 0755);
    struct rlimit unlimited = {RLIM_INFINITY, RLIM_INFINITY};
    check("phase_memlock", setrlimit(RLIMIT_MEMLOCK, &unlimited) == 0);
    cpu_set_t collector; CPU_ZERO(&collector); CPU_SET(1, &collector);
    check("collector_cpu", sched_setaffinity(0, sizeof(collector), &collector) == 0);
    const char *groups[] = {"foreground", "background"};
    pid_t workers[2];
    for (unsigned i = 0; i < 2; i++) {
        char group[160]; snprintf(group, sizeof(group), "/sys/fs/cgroup/%s", groups[i]); mkdir(group, 0755);
        workers[i] = fork();
        if (!workers[i]) {
            cpu_set_t mask; CPU_ZERO(&mask); CPU_SET(0, &mask);
            if (sched_setaffinity(0, sizeof(mask), &mask)) _exit(20);
            char file[192]; snprintf(file, sizeof(file), "%s/cgroup.procs", group);
            if (put(file, "0")) _exit(21);
            for (;;) __asm__ volatile("pause" ::: "memory");
        }
        check("phase_worker_fork", workers[i] > 0);
    }
    usleep(100000);
    int fd = open("/run/cerynth/phase.json", O_WRONLY | O_CREAT | O_TRUNC, 0600);
    check("snapshot_created", fd >= 0);
    if (fd >= 0) {
        dprintf(fd, "{\"version\":1,\"members\":[");
        for (unsigned i = 0; i < 2; i++)
            dprintf(fd, "%s{\"session_id\":\"11111111-1111-4111-8111-%012u\",\"pid\":%d,\"start_time_ticks\":%llu,\"control_group\":\"/%s\",\"phase\":\"%s\"}",
                i ? "," : "", i + 1, workers[i], (unsigned long long)field(workers[i], 22), groups[i], i ? "batch" : "tool");
        dprintf(fd, "]}\n"); close(fd);
    }
    const char *names[] = {"linux-default", "frozen-balanced", "phase-snapshot"};
    for (unsigned trial = 0; trial < 5; trial++) {
        for (unsigned order = 0; order < 3; order++) {
            unsigned mode = trial % 2 ? 2 - order : order;
            pid_t scheduler = 0; int status = 0;
            if (mode) {
                scheduler = phase_scheduler(mode == 1 ? "/usr/lib/cerynth/cerynth-scx-baseline" : "/usr/lib/cerynth/cerynth-scx-phase", mode == 2);
                if (!await_state("enabled\n")) { check("phase_scheduler_attach", 0); goto cleanup; }
            }
            uint64_t start[2];
            for (unsigned i = 0; i < 2; i++) start[i] = field(workers[i], 14) + field(workers[i], 15);
            sleep(3);
            uint64_t ticks[2];
            for (unsigned i = 0; i < 2; i++) ticks[i] = field(workers[i], 14) + field(workers[i], 15) - start[i];
            printf("CERYNTH_PHASE_SAMPLE {\"trial\":%u,\"mode\":\"%s\",\"foreground_ticks\":%llu,\"background_ticks\":%llu}\n",
                trial, names[mode], (unsigned long long)ticks[0], (unsigned long long)ticks[1]);
            check("both_session_workers_make_progress", ticks[0] > 0 && ticks[1] > 0);
            if (scheduler > 0) {
                kill(scheduler, SIGTERM);
                check("phase_linux_fallback", await_state("disabled\n") && reap_bounded(scheduler, &status));
                while (waitpid(-1, NULL, WNOHANG) > 0) {}
            }
        }
    }
    chmod("/run/cerynth/phase.json", 0644);
    pid_t invalid = phase_scheduler("/usr/lib/cerynth/cerynth-scx-phase", 1); int status = 0;
    check("untrusted_snapshot_refused_before_attach", reap_bounded(invalid, &status) && WIFEXITED(status) && WEXITSTATUS(status) != 0 && scheduler_state("disabled\n"));
    chmod("/run/cerynth/phase.json", 0600);
    fd = open("/run/cerynth/phase.json", O_WRONLY | O_TRUNC);
    if (fd >= 0) {
        dprintf(fd, "{\"version\":1,\"members\":[{\"session_id\":\"11111111-1111-4111-8111-111111111111\",\"pid\":%d,\"start_time_ticks\":0,\"control_group\":\"/foreground\",\"phase\":\"tool\"}]}\n", workers[0]);
        close(fd);
    }
    invalid = phase_scheduler("/usr/lib/cerynth/cerynth-scx-phase", 1);
    check("stale_process_witness_refused_before_attach", reap_bounded(invalid, &status) && WIFEXITED(status) && WEXITSTATUS(status) != 0 && scheduler_state("disabled\n"));
cleanup:
    for (unsigned i = 0; i < 2; i++) if (workers[i] > 0) { kill(workers[i], SIGKILL); waitpid(workers[i], NULL, 0); }
    finish(); return 0;
}
