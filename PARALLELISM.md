# Rayon Parallelism Guide

This guide explains how the DCFR Solver uses [Rayon](https://github.com/rayon-rs/rayon) for parallelism, which input parameters affect parallel performance, and how to choose the right `par_decision_depth` setting for your solve.

---

## Table of Contents

- [How Parallelism Works](#how-parallelism-works)
- [The `par_decision_depth` Setting](#the-par_decision_depth-setting)
- [Input Parameters That Affect Parallelism](#input-parameters-that-affect-parallelism)
- [Choosing the Right Setting](#choosing-the-right-setting)
- [Controlling Thread Count](#controlling-thread-count)
- [Benchmarking Your Setup](#benchmarking-your-setup)
- [Troubleshooting](#troubleshooting)

---

## How Parallelism Works

The solver uses Rayon in **three distinct places**, each with different characteristics:

### 1. Chance Node Evaluation (always parallel)

**Location:** `chance_utility()` in `src/cfr.rs`

Every time the solver deals a new board card (flop→turn, turn→river), it traverses all possible next cards in parallel. This is **always on** — there is no setting to disable it.

```
Chance node (flop → turn)
├── Card 1 subtree  ─┐
├── Card 2 subtree   ├── parallel via par_iter()
├── ...              │
└── Card N subtree  ─┘
```

**Why it's safe:** Each card leads to a disjoint subtree with non-overlapping regret memory (guaranteed by depth-first tree construction). Threads never write to the same memory.

**Impact:** This is the largest source of parallelism for multi-street solves (flop, turn). River-only solves have no chance nodes, so they get **zero benefit** from this.

### 2. DCFR Discounting (always parallel)

**Location:** `discount_all()` in `src/cfr.rs`

After every iteration, DCFR applies discount factors to all regret and cumulative strategy data across all decision nodes. This is parallelized via `par_iter()` / `par_iter_mut()`.

**Impact:** Proportional to the number of decision nodes. Large trees (flop with many bet sizes) benefit more. This runs once per iteration and is typically a small fraction of total time (~5-10%).

### 3. Decision Node Action Fan-Out (optional, controlled by `par_decision_depth`)

**Location:** `cfr_traverse()` in `src/cfr.rs`

At each decision node, the solver can traverse all action children (check, bet 33%, bet 67%, etc.) in parallel instead of sequentially. This is the **most impactful** parallelism point but also has the highest overhead.

```
Decision node (OOP's turn)
├── Check subtree     ─┐
├── Bet 33% subtree    ├── parallel via into_par_iter()
├── Bet 67% subtree    │   (only if par_decision_depth enabled)
├── Bet 125% subtree   │
└── All-in subtree    ─┘
```

**Why it's optional:** Small subtrees (deep in the tree) don't have enough work to justify the overhead of spawning Rayon tasks. The `par_decision_depth` cutoff controls where to switch from parallel to sequential.

---

## The `par_decision_depth` Setting

The `par_decision_depth` field in `SubgameConfig` controls decision-node parallelism. It is a **depth cutoff**: decision nodes at depth `< par_decision_depth` parallelize their action children; deeper nodes run sequentially.

### Values

| Value | Behavior | Use Case |
|-------|----------|----------|
| `u32::MAX` (default) | **Disabled** — all decision nodes sequential | Safe default, backward-compatible |
| `0` | Parallelize **root decision node only** | Minimal overhead, helps wide root nodes |
| `1` | Parallelize root + first-level decision nodes | Turn/river solves with multiple bet sizes |
| `2` | Parallelize root + 2 levels of decision nodes | Flop solves with 3+ bet/raise sizes |
| `3` | Parallelize root + 3 levels | Large flop trees with deep betting sequences |
| `4+` | Aggressive parallelism | Very large trees, many-core machines (16+ cores) |

### How Depth Is Counted

`decision_depth` counts **Decision nodes only** — chance nodes are transparent (they pass through the depth counter unchanged).

```
Root decision (depth 0)     ← OOP's first action
├── Check → Chance (flop→turn, depth still 0)
│   └── Decision (depth 1)  ← OOP's turn action
│       └── Decision (depth 2) ← OOP's river action
├── Bet 33% → Decision (depth 1)  ← IP's response
│   └── ...
└── Bet 67% → Decision (depth 1)
    └── ...
```

### Current Status

The CLI currently hardcodes `par_decision_depth: u32::MAX` (disabled). To enable it, you need to modify `src/main.rs` or use the library API directly:

```rust
// In src/main.rs, line ~931:
par_decision_depth: u32::MAX,  // change to 0, 1, 2, etc.
```

Or via the library API:

```rust
use dcfr_solver::cfr::{SubgameConfig, SubgameSolver};

let mut config = SubgameConfig {
    // ... your config ...
    par_decision_depth: 2,  // enable parallel decision fan-out
};
```

---

## Input Parameters That Affect Parallelism

### Parameters That Increase Parallelism Opportunity

| Parameter | Effect | Why |
|-----------|--------|-----|
| **`--street flop`** | 🔺 High | Three streets = two chance nodes (flop→turn, turn→river), each with ~45-48 parallel children. Deepest trees. |
| **`--street turn`** | 🔸 Medium | One chance node (turn→river) with ~46-48 parallel children. |
| **`--street river`** | 🔻 Low | No chance nodes. Only decision fan-out and discounting can parallelize. |
| **More `--bet-sizes`** | 🔺 High | More actions per node = wider fan-out = more parallel work per decision node. E.g., `33,67,125` (3 sizes) vs `67` (1 size). |
| **More `--raise-sizes`** | 🔺 High | Same as bet sizes — wider raise trees. |
| **Higher `--max-raises`** | 🔺 Medium | Deeper betting sequences = more decision nodes = more discount parallelism. |
| **Larger ranges** | 🔸 Medium | More live combos = more work per subtree = better amortization of parallel overhead. |
| **More `--iterations`** | 🔸 Low | More discount_all calls, but per-iteration parallelism is unchanged. |
| **`--no-iso`** | 🔺 High | Disables suit isomorphism → enumerates all ~48 cards instead of ~12 canonical. Much more chance-node parallelism, but also much more total work (net slower). |

### Parameters That Decrease Parallelism Opportunity

| Parameter | Effect | Why |
|-----------|--------|-----|
| **`--pruning`** | 🔻 Low | Skips zero-strategy subtrees, reducing work. Good for speed but reduces parallel work available. |
| **Smaller ranges** | 🔻 Low | Fewer live combos = less work per subtree = parallel overhead may dominate. |
| **`--depth-limit turn`** | 🔻 Medium | Replaces river chance node with NN leaf — removes a parallelism point. |
| **`--geometric`** | 🔻 Low | Reduces bet sizes near the cap, narrowing the tree at deeper levels. |

### Parameters That Don't Affect Parallelism

| Parameter | Why |
|-----------|-----|
| `--pot`, `--stack` | Affect pot sizes, not tree structure. |
| `--no-dcfr` | Disables discounting (removes one parallelism point), but discounting is a small fraction of time. |
| `--skip-cum-strategy` | Saves memory, doesn't change tree shape. |
| `--rake`, `--rake-cap` | Affect payoffs, not tree structure. |
| `--seed` | No effect on parallelism. |
| `--exploration-eps`, `--entropy-bonus`, etc. | Affect strategy computation, not tree structure. |

---

## Choosing the Right Setting

### Decision Tree

```
What street are you solving?
│
├── River only
│   ├── No chance nodes → parallelism is limited to decision fan-out
│   ├── par_decision_depth = 1 (root + first response)
│   └── If <4 cores: keep u32::MAX (disabled)
│
├── Turn
│   ├── One chance node (~46 parallel children) → already gets good parallelism
│   ├── par_decision_depth = 1 (modest decision fan-out boost)
│   └── If 8+ cores and 3+ bet sizes: try par_decision_depth = 2
│
└── Flop
    ├── Two chance nodes → excellent baseline parallelism
    ├── par_decision_depth = 2 (root + 2 levels of decision nodes)
    └── If 16+ cores and 3+ bet/raise sizes: try par_decision_depth = 3
```

### Recommended Settings by Scenario

#### River-only solves (`--street river`)

| Cores | Bet sizes | `par_decision_depth` | Notes |
|-------|-----------|----------------------|-------|
| 1-4 | any | `u32::MAX` (disabled) | No benefit — overhead dominates |
| 4-8 | 1-2 | `u32::MAX` (disabled) | Too few actions to parallelize |
| 4-8 | 3+ | `0` (root only) | Parallelize root action fan-out |
| 8+ | 3+ | `1` | Root + first response level |

**Why:** River solves have no chance nodes, so the only parallelism is decision fan-out and discounting. With few bet sizes, there aren't enough action children to justify parallel overhead.

#### Turn solves (`--street turn`)

| Cores | Bet sizes | `par_decision_depth` | Notes |
|-------|-----------|----------------------|-------|
| 1-4 | any | `u32::MAX` (disabled) | Chance node parallelism is sufficient |
| 4-8 | 1-2 | `u32::MAX` (disabled) | Chance node covers it |
| 4-8 | 3+ | `1` | Boost decision fan-out at top levels |
| 8+ | 3+ | `1` or `2` | More aggressive fan-out |
| 16+ | 3+ | `2` | Full parallelism for wide trees |

**Why:** Turn solves have one chance node (~46 parallel children), which already provides good parallelism on 4+ cores. Decision fan-out is a secondary boost.

#### Flop solves (`--street flop`)

| Cores | Bet/Raise sizes | `par_decision_depth` | Notes |
|-------|-----------------|----------------------|-------|
| 1-4 | any | `u32::MAX` (disabled) | Chance nodes provide enough parallelism |
| 4-8 | 1-2 | `u32::MAX` (disabled) | Two chance nodes already saturate cores |
| 4-8 | 3+ | `1` | Modest decision fan-out boost |
| 8-16 | 3+ | `2` | Good balance for wide flop trees |
| 16+ | 3+ | `2` or `3` | Aggressive parallelism for large trees |
| 32+ | 3+ | `3` | Saturate many cores with deep fan-out |

**Why:** Flop solves have two chance nodes (flop→turn→river), each with ~45-48 canonical children. This provides excellent baseline parallelism. Decision fan-out helps when you have many bet sizes and many cores.

#### Batch runs (`batch-run`)

Batch runs solve many independent spots sequentially. Each spot uses the solver's internal parallelism. For batch runs:

- **Don't set `par_decision_depth` too high** — each spot is small, and parallel overhead can dominate.
- **Consider running multiple batch instances in parallel** instead (using `--start` and `--count` to split work), with each instance using fewer threads.

```bash
# Instead of one process with 16 threads:
RAYON_NUM_THREADS=4 cargo run --release -- batch-run --input configs.jsonl --start 0 --count 500 &
RAYON_NUM_THREADS=4 cargo run --release -- batch-run --input configs.jsonl --start 500 --count 500 &
RAYON_NUM_THREADS=4 cargo run --release -- batch-run --input configs.jsonl --start 1000 --count 500 &
RAYON_NUM_THREADS=4 cargo run --release -- batch-run --input configs.jsonl --start 1500 --count 500 &
```

This is often faster than one process with 16 threads because each spot has limited parallelism opportunity.

---

## Controlling Thread Count

### Rayon Global Thread Pool

The solver calls `init_rayon_pool()` which creates a global Rayon thread pool with **16MB stack size** per thread (needed for deep CFR recursion with large stack arrays).

By default, Rayon uses **one thread per logical CPU core**. You can override this with the `RAYON_NUM_THREADS` environment variable:

```bash
# Use 8 threads instead of all cores
RAYON_NUM_THREADS=8 cargo run --release -- solve --board "AsKs3d7c2h" --street river ...

# Use 1 thread (fully sequential, for debugging)
RAYON_NUM_THREADS=1 cargo run --release -- solve --board "AsKs3d7c2h" --street river ...
```

### When to Limit Threads

| Scenario | Recommendation |
|----------|----------------|
| **Memory-constrained** | Each thread uses ~16MB stack + thread-local buffer pools (~2MB). On 32-core machines with limited RAM, reducing to 8-16 threads can prevent memory pressure. |
| **Batch runs** | Run multiple processes with fewer threads each (see above). |
| **Debugging** | `RAYON_NUM_THREADS=1` eliminates concurrency, making race conditions and ordering bugs deterministic. |
| **Shared machine** | Limit to 50-75% of cores to leave capacity for other tasks. |
| **Hyperthreading** | On CPUs where hyperthreading doesn't help (memory-bound workloads), try `RAYON_NUM_THREADS = physical_cores` instead of logical cores. |

### Thread-Local Buffer Pools

The solver uses thread-local buffer pools (`STRAT_POOL`, `UTILS_POOL`, `REGRET_POOL`) to avoid repeated heap allocation in the hot path. Each Rayon thread gets its own pool. This means:

- More threads = more memory for buffer pools (~2MB per thread)
- Buffers are reused across iterations within the same thread
- No contention between threads (each has its own pool)

---

## Benchmarking Your Setup

### Quick Benchmark

Run the same solve with different settings and compare iteration speed:

```bash
# Baseline (parallelism disabled)
RAYON_NUM_THREADS=1 cargo run --release -- solve \
  --board "Td9d6h" --street flop \
  --oop-range "AA-22,AKs-A2s" --ip-range "AA-22,AKs-A2s" \
  --pot 30 --stack 170 --iterations 100 \
  --bet-sizes 33,67,125 --raise-sizes 50,100 \
  --output /dev/null 2>&1 | grep "iter"

# With chance-node parallelism only (default)
cargo run --release -- solve \
  --board "Td9d6h" --street flop \
  --oop-range "AA-22,AKs-A2s" --ip-range "AA-22,AKs-A2s" \
  --pot 30 --stack 170 --iterations 100 \
  --bet-sizes 33,67,125 --raise-sizes 50,100 \
  --output /dev/null 2>&1 | grep "iter"
```

Compare the time per iteration (reported in the callback output). The solver prints iteration progress at doubling intervals.

### What to Measure

| Metric | How to Get It |
|--------|---------------|
| **Iterations/second** | Time the first 100 iterations, divide by elapsed time |
| **Speedup vs 1 thread** | `time_1thread / time_Nthreads` |
| **Parallel efficiency** | `speedup / N_threads` (1.0 = perfect, 0.5 = half efficiency) |
| **Memory usage** | Watch `htop` or Task Manager during the solve |

### Expected Speedups

Based on the solver's parallelism profile:

| Scenario | 4 cores | 8 cores | 16 cores | 32 cores |
|----------|---------|---------|----------|----------|
| River, 1 bet size | ~1.0× | ~1.0× | ~1.0× | ~1.0× |
| River, 3 bet sizes, `par_depth=1` | ~2.5× | ~4× | ~5× | ~5× |
| Turn, 1 bet size | ~3× | ~5× | ~6× | ~6× |
| Turn, 3 bet sizes, `par_depth=1` | ~3.5× | ~6× | ~8× | ~8× |
| Flop, 1 bet size | ~3.5× | ~6× | ~7× | ~7× |
| Flop, 3 bet sizes, `par_depth=2` | ~3.8× | ~7× | ~12× | ~15× |

**Note:** These are estimates. Actual speedups depend on tree size, range width, memory bandwidth, and CPU architecture. Diminishing returns above 8-16 cores are expected because:
1. Memory bandwidth becomes the bottleneck (showdown evaluation is memory-bound)
2. Amdahl's law: the sequential portions (regret matching, strategy computation) limit maximum speedup
3. Chance node parallelism saturates at ~48 children (one per remaining card)

---

## Troubleshooting

### "It's slower with parallelism enabled"

**Cause:** Parallel overhead dominates when subtrees are too small.

**Fix:**
- Use a lower `par_decision_depth` (or disable it with `u32::MAX`)
- Ensure you're solving a large enough tree (flop with 3+ bet sizes, not river with 1 bet size)
- Check that you're running a release build (`cargo build --release`)

### "Stack overflow / segfault"

**Cause:** The default 8MB Rayon stack is too small for deep CFR recursion.

**Fix:** The solver already calls `init_rayon_pool()` with 16MB stacks. If you're using the library API directly, make sure to call it:

```rust
dcfr_solver::cfr::init_rayon_pool();
```

If 16MB is still not enough (very deep trees with `max_raises=4+`), increase it:

```rust
rayon::ThreadPoolBuilder::new()
    .stack_size(32 * 1024 * 1024)  // 32MB
    .build_global()
    .ok();
```

### "High memory usage with many threads"

**Cause:** Each thread allocates its own buffer pools (~2MB) and 16MB stack.

**Fix:** Reduce thread count:
```bash
RAYON_NUM_THREADS=8 cargo run --release -- solve ...
```

### "CPU usage is low (not all cores utilized)"

**Cause:** Not enough parallel work available.

**Fix:**
- For river solves: enable `par_decision_depth` and use more bet sizes
- For small trees: this is expected — the solver is computation-bound, not parallelism-bound
- Check if `--pruning` is skipping too many subtrees

### "Results differ between runs with different thread counts"

**Cause:** This should not happen. The solver uses deterministic regret updates with disjoint memory regions. If you see different results, it's a bug.

**Fix:** Report it. As a workaround, use `RAYON_NUM_THREADS=1` for reproducible debugging.

---

## Summary

| Setting | Default | When to Change |
|---------|---------|----------------|
| `par_decision_depth` | `u32::MAX` (disabled) | Set to `1-3` for wide trees (3+ bet sizes) on 4+ core machines |
| `RAYON_NUM_THREADS` | all cores | Reduce for memory-constrained or shared machines; use `1` for debugging |
| `init_rayon_pool()` stack size | 16MB | Increase to 32MB for very deep trees (`max_raises=4+`) |

**Golden rule:** Start with the default (`par_decision_depth = u32::MAX`). Benchmark. Only enable decision-node parallelism if you see low CPU utilization on a multi-core machine with a wide tree. Chance-node parallelism is always on and provides the bulk of the speedup for multi-street solves.