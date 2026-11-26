/* Native upstream cpu.max experiment; executed only as init in a disposable VM. */
#define main resource_suite_main
static void finish(void) __attribute__((noreturn));
#include "platform_guest.c"
#undef main
#include <sched.h>

static void cpu_file(const char *group, const char *name, const char *setting) {
    char file[256]; snprintf(file, sizeof(file), "/sys/fs/cgroup/%s/%s", group, name);
    check(name, put(file, setting) == 0);
}

static uint64_t usage(const char *group) {
    char file[256]; snprintf(file, sizeof(file), "/sys/fs/cgroup/%s/cpu.stat", group);
    return value(file, "usage_usec");
}

static void sample(const char *name, unsigned trial) {
    uint64_t start = usage("budget"), a = usage("budget/a"), b = usage("budget/b");
    struct timespec before, after; clock_gettime(CLOCK_MONOTONIC, &before);
    sleep(3);
    clock_gettime(CLOCK_MONOTONIC, &after);
    double elapsed = after.tv_sec - before.tv_sec + (after.tv_nsec - before.tv_nsec) / 1e9;
    double used = (usage("budget") - start) / 1e6;
    uint64_t a_used = usage("budget/a") - a, b_used = usage("budget/b") - b;
    printf("CERYNTH_BANDWIDTH_SAMPLE {\"condition\":\"%s\",\"trial\":%u,\"wall_seconds\":%.6f,\"parent_cpu_seconds\":%.6f,\"a_cpu_usec\":%llu,\"b_cpu_usec\":%llu}\n",
           name, trial, elapsed, used, (unsigned long long)(usage("budget/a") - a), (unsigned long long)(usage("budget/b") - b));
    check("both_children_progress", usage("budget/a") > a && usage("budget/b") > b);
    /* A coarse sustained cap check, not a per-period jitter guarantee. */
    double ceiling = !strcmp(name, "parent-20-percent") ? .35 : (!strcmp(name, "parent-60-percent") ? .80 : .45);
    check("sustained_hierarchical_bandwidth_cap", used / elapsed < ceiling && used / elapsed > .05);
    if (!strcmp(name, "child-10-plus-20-percent"))
        check("individual_child_bandwidth_caps", a_used / (elapsed * 1e6) < .17 && b_used / (elapsed * 1e6) < .27);
}

int main(void) {
    if (getpid() != 1) return 2;
    setvbuf(stdout, NULL, _IOLBF, 0);
    mount("devtmpfs", "/dev", "devtmpfs", 0, NULL);
    mount("proc", "/proc", "proc", 0, NULL);
    mount("sysfs", "/sys", "sysfs", 0, NULL);
    mount("debugfs", "/sys/kernel/debug", "debugfs", 0, NULL);
    mkdir("/sys/kernel/tracing", 0755);
    mount("tracefs", "/sys/kernel/tracing", "tracefs", 0, NULL);
    mkdir("/sys/fs/cgroup", 0755); mount("none", "/sys/fs/cgroup", "cgroup2", 0, NULL);
    struct rlimit unlimited = {RLIM_INFINITY, RLIM_INFINITY}; setrlimit(RLIMIT_MEMLOCK, &unlimited);
    check("cpu_controller", put("/sys/fs/cgroup/cgroup.subtree_control", "+cpu") == 0);
    mkdir("/sys/fs/cgroup/budget", 0755);
    cpu_file("budget", "cgroup.subtree_control", "+cpu");
    mkdir("/sys/fs/cgroup/budget/a", 0755); mkdir("/sys/fs/cgroup/budget/b", 0755);
    cpu_file("budget", "cpu.max", "20000 100000");
    char *command_line = get("/proc/cmdline");
    int linux_control = command_line && strstr(command_line, "cerynth.cpu_policy=linux");
    pid_t scheduler = linux_control ? 0 : fork();
    if (!linux_control && !scheduler) {
        execl("/usr/lib/cerynth/scx_lavd", "scx_lavd", "--enable-cpu-bw", "--performance", "--no-freq-scaling", "--no-use-em", NULL);
        _exit(127);
    }
    if (!linux_control && !await_state("enabled\n")) {
        check("upstream_bandwidth_scheduler_attached", 0);
        if (scheduler > 0) { kill(scheduler, SIGKILL); waitpid(scheduler, NULL, 0); }
        finish();
    }
    check(linux_control ? "linux_control_scheduler" : "upstream_bandwidth_scheduler_attached", 1);
    pid_t workers[2] = {0, 0};
    for (unsigned i = 0; i < 2; i++) {
        workers[i] = fork();
        if (!workers[i]) {
            char group[128]; snprintf(group, sizeof(group), "/sys/fs/cgroup/budget/%c/cgroup.procs", 'a' + i);
            if (put(group, "0")) _exit(10);
            for (;;) __asm__ volatile("pause" ::: "memory");
        }
        check("worker_created", workers[i] > 0);
    }
    for (unsigned trial = 0; trial < 3; trial++) {
        cpu_file("budget", "cpu.max", "20000 100000"); sleep(1); sample("parent-20-percent", trial);
        cpu_file("budget", "cpu.max", "60000 100000"); sleep(1); sample("parent-60-percent", trial);
        cpu_file("budget/a", "cpu.max", "10000 100000");
        cpu_file("budget/b", "cpu.max", "20000 100000"); sleep(1); sample("child-10-plus-20-percent", trial);
        cpu_file("budget/a", "cpu.max", "max 100000"); cpu_file("budget/b", "cpu.max", "max 100000");
    }
    for (unsigned i = 0; i < 2; i++) if (workers[i] > 0) { kill(workers[i], SIGKILL); waitpid(workers[i], NULL, 0); }
    if (scheduler > 0) {
        kill(scheduler, SIGTERM); int status = 0;
        check("upstream_exit_linux_fallback", await_state("disabled\n") && reap_bounded(scheduler, &status));
    }
    finish();
}
