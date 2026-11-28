/* Runs only as PID 1 inside the disposable platform validation VM. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/resource.h>
#include <sys/reboot.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/utsname.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

static const char *base = "/sys/fs/cgroup/cerynth/parent/child";
static unsigned failures;

static void check(const char *name, int success) {
    printf("CERYNTH_CHECK {\"name\":\"%s\",\"passed\":%s}\n", name, success ? "true" : "false");
    fflush(stdout);
    failures += !success;
}

static int put(const char *path, const char *value) {
    int fd = open(path, O_WRONLY | O_CLOEXEC);
    if (fd < 0) return -1;
    size_t size = strlen(value);
    ssize_t written = write(fd, value, size);
    close(fd);
    return written == (ssize_t)size ? 0 : -1;
}

static char *get(const char *path) {
    static char data[8192];
    int fd = open(path, O_RDONLY | O_CLOEXEC);
    if (fd < 0) return NULL;
    ssize_t count = read(fd, data, sizeof(data) - 1);
    close(fd);
    if (count < 0) return NULL;
    data[count] = 0;
    return data;
}

static uint64_t value(const char *path, const char *key) {
    char *text = get(path);
    if (!text) return 0;
    if (key) {
        text = strstr(text, key);
        if (!text) return 0;
        text += strlen(key);
    }
    return strtoull(text, NULL, 10);
}

static void path(char *out, const char *name) {
    snprintf(out, 512, "%s/%s", base, name);
}

static int join(void) {
    char file[512];
    path(file, "cgroup.procs");
    return put(file, "0");
}

static int scheduler_state(const char *expected) {
    char *text = get("/sys/kernel/sched_ext/state");
    return text && !strcmp(text, expected);
}

static int await_state(const char *expected) {
    for (unsigned i = 0; i < 100; i++) {
        if (scheduler_state(expected)) return 1;
        usleep(100000);
    }
    return 0;
}

static pid_t launch_scheduler_value(const char *profile, const char *fault, const char *setting) {
    pid_t child = fork();
    if (!child) {
        if (fault) setenv(fault, setting, 1);
        execl("/usr/lib/cerynth/cerynth-scx", "cerynth-scx", "--profile", profile, NULL);
        perror("exec scheduler");
        _exit(127);
    }
    return child;
}

static pid_t launch_scheduler(const char *profile, const char *fault) {
    return launch_scheduler_value(profile, fault, "1");
}

static int reap_bounded(pid_t child, int *status) {
    if (child <= 0) return 0;
    for (unsigned i = 0; i < 100; i++) {
        if (waitpid(child, status, WNOHANG) == child) return 1;
        usleep(100000);
    }
    kill(child, SIGKILL);
    waitpid(child, status, 0);
    return 0;
}

static void fault_tests(void) {
    const char *faults[] = {"crash-sigabrt", "status-deleted", "status-corrupt", "heartbeat-freeze", "dispatch-errors", "dispatch-stall", "sysrq-disable"};
    pid_t workers[3];
    for (unsigned w = 0; w < 3; w++) {
        workers[w] = fork();
        if (!workers[w]) for (;;) __asm__ volatile("pause" ::: "memory");
    }
    for (unsigned f = 0; f < 7; f++) {
        printf("CERYNTH_FAULT %s\n", faults[f]);
        int status = 0;
        uint64_t sequence = value("/sys/kernel/sched_ext/enable_seq", NULL);
        pid_t scheduler = launch_scheduler_value("balanced", f == 4 || f == 5 ? "CERYNTH_SCX_INJECT_DISPATCH_ERRORS" : NULL, f == 4 ? "50" : "1");
        if (f == 5) {
            check("stalled_scheduler_exits", reap_bounded(scheduler, &status));
            check("stalled_scheduler_loaded_before_exit", value("/sys/kernel/sched_ext/enable_seq", NULL) > sequence);
            check("stalled_scheduler_reports_failure", WIFEXITED(status) && WEXITSTATUS(status) != 0);
        } else {
            check("fault_scheduler_attached", scheduler > 0 && await_state("enabled\n"));
            sleep(1);
            if (f == 0) { if (scheduler > 0) kill(scheduler, SIGABRT); }
            if (f == 1 || f == 2) {
                uint64_t before = value("/run/cerynth/scheduler-stats.json", "\"progress_sequence\":");
                if (f == 1) { unlink("/run/cerynth/scheduler.json"); unlink("/run/cerynth/scheduler-stats.json"); }
                else { put("/run/cerynth/scheduler.json", "invalid JSON"); put("/run/cerynth/scheduler-stats.json", "invalid JSON"); }
                sleep(3);
                char *restored = get("/run/cerynth/scheduler.json");
                int json_prefix = restored && restored[strspn(restored, " \r\n\t")] == '{';
                uint64_t heartbeat = value("/run/cerynth/scheduler.json", "\"heartbeat\":");
                check("reporter_restores_status_identity", json_prefix && value("/run/cerynth/scheduler.json", "\"pid\":") == (uint64_t)scheduler && heartbeat > 0 && (uint64_t)time(NULL) - heartbeat <= 2);
                check("reporter_restores_stats_progress", value("/run/cerynth/scheduler-stats.json", "\"progress_sequence\":") > before);
                check("reporter_fault_keeps_scheduler_enabled", scheduler_state("enabled\n"));
            }
            if (f == 3) {
                if (scheduler > 0) kill(scheduler, SIGSTOP);
                check("kernel_watchdog_ejects_frozen_scheduler", await_state("disabled\n"));
                if (scheduler > 0) kill(scheduler, SIGCONT);
            }
            if (f == 4) {
                sleep(5);
                check("dispatch_errors_counted", value("/run/cerynth/scheduler-stats.json", "\"dispatch_errors\":") > 0);
                check("dispatch_retries_preserve_progress", scheduler_state("enabled\n") && value("/run/cerynth/scheduler-stats.json", "\"tasks_dispatched\":") > 0);
                check("dispatch_pending_bounded", value("/run/cerynth/scheduler-stats.json", "\"tasks_pending\":") <= 128);
            }
            if (f == 6) check("sysrq_disable_accepted", put("/proc/sysrq-trigger", "S") == 0);
            if (f == 1 || f == 2 || f == 4) { if (scheduler > 0) kill(scheduler, SIGTERM); }
            check("fault_scheduler_reaped", reap_bounded(scheduler, &status));
        }
        check("fault_returns_to_linux", await_state("disabled\n"));
        while (waitpid(-1, NULL, WNOHANG) > 0) {}
        pid_t recovery = launch_scheduler("balanced", NULL);
        check("fresh_scheduler_after_fault", recovery > 0 && await_state("enabled\n"));
        sleep(1);
        check("fresh_scheduler_identity_after_fault", value("/run/cerynth/scheduler.json", "\"pid\":") == (uint64_t)recovery);
        if (recovery > 0) kill(recovery, SIGTERM);
        check("recovery_scheduler_reaped", reap_bounded(recovery, &status));
        check("recovery_returns_to_linux", await_state("disabled\n"));
        while (waitpid(-1, NULL, WNOHANG) > 0) {}
    }
    for (unsigned w = 0; w < 3; w++) if (workers[w] > 0) { kill(workers[w], SIGKILL); waitpid(workers[w], NULL, 0); }
}

static void scheduler_tests(void) {
    if (access("/usr/lib/cerynth/cerynth-scx", X_OK)) return;
    mkdir("/run", 0755); mkdir("/run/cerynth", 0755);
    struct rlimit limit = {RLIM_INFINITY, RLIM_INFINITY};
    check("scheduler_memlock", setrlimit(RLIMIT_MEMLOCK, &limit) == 0);
    const char *profiles[] = {"balanced", "interactive", "performance", "background"};
    for (unsigned p = 0; p < 4; p++) {
        printf("CERYNTH_PROFILE %s\n", profiles[p]);
        pid_t workers[3];
        for (unsigned w = 0; w < 3; w++) {
            workers[w] = fork();
            if (!workers[w]) for (;;) __asm__ volatile("pause" ::: "memory");
        }
        pid_t scheduler = launch_scheduler(profiles[p], NULL);
        int attached = scheduler > 0 && await_state("enabled\n");
        check("scheduler_attached", attached);
        if (attached) {
            sleep(1);
            uint64_t before = value("/run/cerynth/scheduler-stats.json", "\"progress_sequence\":");
            sleep(2);
            uint64_t after = value("/run/cerynth/scheduler-stats.json", "\"progress_sequence\":");
            check("scheduler_progress_under_contention", after > before);
            uint64_t heartbeat = value("/run/cerynth/scheduler.json", "\"heartbeat\":");
            check("scheduler_fresh_heartbeat", heartbeat > 0 && (uint64_t)time(NULL) - heartbeat <= 2);
            uint64_t observed_pid = value("/run/cerynth/scheduler.json", "\"pid\":");
            char *text = get("/run/cerynth/scheduler.json");
            check("scheduler_process_profile_identity", observed_pid == (uint64_t)scheduler && text && strstr(text, profiles[p]));
        }
        int status = 0;
        if (scheduler > 0) kill(scheduler, p == 3 ? SIGKILL : SIGTERM);
        check(p == 3 ? "sigkill_linux_fallback" : "clean_stop_linux_fallback", await_state("disabled\n"));
        check("scheduler_child_reaped", reap_bounded(scheduler, &status));
        printf("CERYNTH_SCHEDULER_EXIT {\"profile\":\"%s\",\"wait_status\":%d}\n", profiles[p], status);
        for (unsigned w = 0; w < 3; w++) if (workers[w] > 0) { kill(workers[w], SIGKILL); waitpid(workers[w], NULL, 0); }
        while (waitpid(-1, NULL, WNOHANG) > 0) {} /* adopted reporter descendants */
        if (!scheduler_state("disabled\n")) break;
    }
    pid_t injected = launch_scheduler("balanced", "CERYNTH_SCX_INJECT_LOAD_FAILURE");
    int status = 0;
    check("injected_load_failure_exits_75", reap_bounded(injected, &status) && WIFEXITED(status) && WEXITSTATUS(status) == 75);
    check("load_failure_linux_fallback", await_state("disabled\n"));
    while (waitpid(-1, NULL, WNOHANG) > 0) {}
    fault_tests();
}

static void finish(void) {
    printf("CERYNTH_RESULT {\"version\":1,\"failures\":%u,\"scope\":\"disposable-kernel-vm\"}\n", failures);
    fflush(NULL);
    sync();
    reboot(RB_POWER_OFF);
    _exit(failures ? 1 : 0);
}

int main(void) {
    if (getpid() != 1) {
        fprintf(stderr, "This program must run only as VM init.\n");
        return 2;
    }
    setvbuf(stdout, NULL, _IONBF, 0);
    mkdir("/proc", 0755); mkdir("/sys", 0755); mkdir("/dev", 0755);
    check("mount_proc", mount("proc", "/proc", "proc", 0, NULL) == 0);
    check("mount_sys", mount("sysfs", "/sys", "sysfs", 0, NULL) == 0);
    check("mount_dev", mount("devtmpfs", "/dev", "devtmpfs", 0, NULL) == 0);
    struct utsname kernel;
    uname(&kernel);
    printf("CERYNTH_KERNEL %s\n", kernel.release);
    check("mount_cgroup2", mount("none", "/sys/fs/cgroup", "cgroup2", 0, NULL) == 0);
    char *state = get("/sys/kernel/sched_ext/state");
    check("linux_fallback_initially_disabled_scx", state && !strcmp(state, "disabled\n"));
    check("enable_controllers", put("/sys/fs/cgroup/cgroup.subtree_control", "+cpu +memory +pids +io +cpuset") == 0);
    const char *groups[] = {"/sys/fs/cgroup/cerynth", "/sys/fs/cgroup/cerynth/parent", "/sys/fs/cgroup/cerynth/parent/child"};
    for (unsigned i = 0; i < 3; i++) {
        check("create_hierarchy", mkdir(groups[i], 0755) == 0);
        if (i < 2) {
            char file[512]; snprintf(file, sizeof(file), "%s/cgroup.subtree_control", groups[i]);
            check("delegate_controllers", put(file, "+cpu +memory +pids +io +cpuset") == 0);
        }
    }
    check("parent_memory_max", put("/sys/fs/cgroup/cerynth/parent/memory.max", "100663296") == 0);
    char file[512];
    path(file, "memory.max"); check("child_memory_max", put(file, "67108864") == 0 && value(file, NULL) == 67108864);
    path(file, "memory.high"); check("memory_high", put(file, "50331648") == 0 && value(file, NULL) == 50331648);
    path(file, "memory.swap.max"); check("swap_disabled", put(file, "0") == 0 && value(file, NULL) == 0);
    path(file, "cpu.weight"); check("cpu_weight_readback_only", put(file, "150") == 0 && value(file, NULL) == 150);
    path(file, "cpu.max"); check("cpu_quota", put(file, "10000 100000") == 0);
    path(file, "io.weight");
    int io_set = put(file, "default 100") == 0;
    char *io_value = get(file);
    check("io_weight_readback_only", io_set && io_value && strstr(io_value, "default 100"));
    int ready[2];
    if (pipe(ready)) { check("worker_pipe", 0); finish(); }
    pid_t worker = fork();
    if (!worker) {
        close(ready[0]);
        if (join()) _exit(3);
        volatile unsigned char *memory = malloc(32 * 1024 * 1024);
        if (!memory) _exit(4);
        for (size_t i = 0; i < 32 * 1024 * 1024; i += 4096) memory[i] = 1;
        if (write(ready[1], "R", 1) != 1) _exit(5);
        close(ready[1]);
        for (;;) __asm__ volatile("pause" ::: "memory");
    }
    close(ready[1]);
    char byte;
    check("worker_ready", worker > 0 && read(ready[0], &byte, 1) == 1);
    close(ready[0]);
    path(file, "memory.current"); uint64_t child_memory = value(file, NULL);
    uint64_t parent_memory = value("/sys/fs/cgroup/cerynth/parent/memory.current", NULL);
    check("ancestor_memory_accounting", child_memory >= 32 * 1024 * 1024 && parent_memory >= child_memory);
    sleep(2);
    if (worker > 0) { kill(worker, SIGKILL); waitpid(worker, NULL, 0); }
    path(file, "cpu.stat"); uint64_t cpu = value(file, "usage_usec");
    uint64_t throttled = value(file, "nr_throttled");
    printf("CERYNTH_CPU {\"usage_usec\":%llu,\"nr_throttled\":%llu}\n", (unsigned long long)cpu, (unsigned long long)throttled);
    check("cpu_quota_actually_throttles", throttled > 0 && cpu > 30000 && cpu < 800000);
    path(file, "pids.max"); check("pids_limit", put(file, "1") == 0);
    pid_t limited = fork();
    if (!limited) {
        if (join()) _exit(3);
        pid_t unexpected = fork();
        if (unexpected < 0) _exit(errno == EAGAIN ? 0 : 4);
        if (!unexpected) _exit(0);
        waitpid(unexpected, NULL, 0); _exit(5);
    }
    int status = 0;
    if (limited > 0) waitpid(limited, &status, 0);
    check("pids_fork_denied", limited > 0 && WIFEXITED(status) && WEXITSTATUS(status) == 0);
    path(file, "pids.events"); check("pids_event_recorded", value(file, "max") > 0);
    // The high watermark was read back; this is not a pressure-policy benchmark.
    // Remove reclaim throttling and CPU bandwidth for the separate hard-OOM
    // test so it measures containment rather than unbounded reclaim latency.
    path(file, "memory.high"); check("oom_test_high_reset", put(file, "max") == 0);
    path(file, "cpu.max"); check("oom_test_quota_reset", put(file, "max 100000") == 0);
    pid_t oom = fork();
    if (!oom) {
        if (join()) _exit(3);
        volatile unsigned char *memory = malloc(128 * 1024 * 1024);
        if (!memory) _exit(4);
        for (size_t i = 0; i < 128 * 1024 * 1024; i += 4096) memory[i] = 1;
        _exit(5);
    }
    if (oom > 0) waitpid(oom, &status, 0);
    path(file, "memory.events");
    check("memory_oom_contained", oom > 0 && WIFSIGNALED(status) && WTERMSIG(status) == SIGKILL && value(file, "oom_kill") > 0);
    path(file, "memory.pressure"); check("memory_pressure_available", get(file) != NULL);
    path(file, "cpu.pressure"); check("cpu_pressure_available", get(file) != NULL);
    path(file, "io.pressure"); check("io_pressure_available", get(file) != NULL);
    path(file, "cgroup.events"); check("no_descendant_processes", value(file, "populated") == 0);
    for (int i = 2; i >= 0; i--) check("hierarchy_cleanup", rmdir(groups[i]) == 0);
    check("init_survived_child_oom", getpid() == 1);
    scheduler_tests();
    finish();
}
