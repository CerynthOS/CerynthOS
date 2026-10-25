/* Host regression harness for the actual production drain callback. */
#include <stdlib.h>
#include <stddef.h>
#include <stdio.h>
#include <string.h>

#define CHECK(expr) do { if (!(expr)) { fprintf(stderr, "check failed: %s\n", #expr); exit(1); } } while (0)

struct dispatched_task_ctx { unsigned int pid; };
struct bpf_dynptr { const struct dispatched_task_ctx *task; };
static unsigned int slots, delivered[1024];
static void *bpf_dynptr_data(struct bpf_dynptr *p, unsigned int off, unsigned int size)
{
    CHECK(off == 0 && size == sizeof(struct dispatched_task_ctx));
    return (void *)p->task;
}
static void dispatch_task(const struct dispatched_task_ctx *task)
{
    CHECK(slots > 0); /* No task may be consumed beyond dispatch capacity. */
    CHECK(task->pid < 1024);
    --slots;
    ++delivered[task->pid];
}
static unsigned int scx_bpf_dispatch_nr_slots(void) { return slots; }
#include "dispatch.h"

/* Model the kernel's documented callback contract, not its scheduler policy. */
static unsigned int drain(unsigned int first, unsigned int remaining, unsigned int capacity)
{
    unsigned int n;
    slots = capacity;
    for (n = 0; n < remaining; ++n) {
        struct dispatched_task_ctx task = { first + n };
        struct bpf_dynptr record = { &task };
        long stop = handle_dispatched_task(&record, NULL);
        CHECK(stop == 0 || stop == 1);
        if (stop) return n + 1;
    }
    return n;
}
int main(void)
{
    /* A burst must fill available slots, rather than stopping after one. */
    CHECK(drain(0, 64, 32) == 32);
    CHECK(drain(32, 32, 32) == 32);
    for (unsigned int i = 0; i < 64; ++i) CHECK(delivered[i] == 1);
    memset(delivered, 0, sizeof(delivered));
    /* Exhaustion must stop before consuming another task. */
    CHECK(drain(0, 8, 1) == 1);
    CHECK(delivered[0] == 1 && delivered[1] == 0);
    memset(delivered, 0, sizeof(delivered));
    for (unsigned int round = 0; round < 100; ++round) {
        unsigned int done = 0;
        while (done < 257) done += drain(done, 257 - done, 16);
    }
    for (unsigned int i = 0; i < 257; ++i) CHECK(delivered[i] == 100);
    struct bpf_dynptr empty = { NULL };
    CHECK(handle_dispatched_task(&empty, NULL) == 0);
    puts("production callback: burst draining, capacity limit, 100 rounds and exact-once delivery passed");
}
