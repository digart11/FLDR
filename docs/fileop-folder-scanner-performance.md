# Folder-statistics enumeration optimization

## Baseline and scope

Source baseline: `8da02d8b5d04965496a0bdec17c6a824e0e65075`, Group 5.0.1, package version 0.1.0. Existing Cargo configuration and release-artifact differences were preserved. No JPEG, search, Copy Manager, UI, cache, scheduler or transfer-engine changes were made.

Before editing either production file, added the standalone debug example, built it, saved its executable as `before.exe`, created deterministic temporary fixtures, and recorded the original scanner's results in `before.csv`. No existing scanner performance benchmark was found; existing scanner, folder-statistics, persistence and native-enumeration tests were reused.

No Release build or FileOP launch was performed. All Cargo output uses `C:\Build\FileOP`, four jobs, offline resolution and the existing lockfile.

## Exact files changed

- `crates/atlas-core/src/fs/folder_size.rs`: metadata adapter, native/portable directory iterator selection, focused inline regression tests; existing traversal retained.
- `crates/atlas-win-space/src/directory.rs`: `StatisticsCursor`, sharing the existing native record parser. Existing listing and watcher APIs and their implementations are unchanged.
- `crates/atlas-core/examples/folder_stats_bench.rs`: new deterministic benchmark with wall/process-CPU and Rust-heap allocation instrumentation.
- `docs/fileop-folder-scanner-performance.md`: this report.

## Before and after

Previously each recursive child became a path, then `symlink_metadata` obtained its attributes, type and logical size. Directories were enumerated with `read_dir` and results published post-order.

On supported Windows filesystems the scanner now consumes the existing FileIdExtdDirectoryInformation record format, via `NtQueryDirectoryFile` in 64 KiB pages. Native records already supply the attributes, logical end-of-file size and directory flag needed by the scanner. Native children no longer receive a separate pathname-based metadata request. Child paths remain necessary for exclusions, descent, errors and descendant-cache publication.

`StatisticsCursor` is resumable: the scanner retains its original post-order stack, while cursors retain only the decoded current page. A reusable queue avoids allocation churn between pages. All active cursors share one lazily allocated 64 KiB scratch buffer per traversal; no entire-directory result vector or new recursive worker pool is introduced.

Explicit-root metadata checks and canonicalization remain. Each opened native directory handle receives one attribute/tag query. This query is necessary for safety: OPEN_REPARSE_POINT can open a concurrently substituted junction whose enumeration appears empty instead of failing. The handle check prevents publishing a misleading exact zero. The handle is opened without following its final reparse component; no file data stream is opened.

All existing reparse, offline and recall bits (`0x400`, `0x1000`, `0x40000`, `0x400000`) remain excluded. Unknown reparse types are still skipped rather than followed. Enumeration metadata is a best-effort snapshot: a file changed after its parent page was read can contribute its earlier size. The next scan observes its newer size. Neither implementation is a transactional filesystem snapshot.

Unsupported native information classes/providers fall back to the existing portable enumeration only before any native child has been returned (Win32 errors 1, 50, 87, 124). The fallback rechecks the directory's reparse/offline status on Windows. Access, sharing, network, malformed-page and mid-enumeration errors remain errors; there is no restart that could double-count a partially consumed directory. Non-Windows traversal retains its portable path and per-entry metadata behavior.

The original 256-frame depth limit, explicit-root overlap handling, Files/Folders counts, descendant aliases and publication, partial/error/cancel classifications, 150 ms progress cadence and cancellation checks remain. The automatic wrapper is unchanged: same roots, worker admission, storage policies, priorities and pacing. No new automatic scans, prefetch trees or cache requests are scheduled. One native page may already have been decoded when cancellation is observed.

## Measurement method

Fixtures are under `%TEMP%\fileop-stats-20261117-084434\fixtures`, outside the project:

- `small`: 12,000 files of 32 bytes, one directory; expected 384,000 bytes, 12,000 files, 0 descendant folders, 1 directory result.
- `nested`: 8 branches containing 128 leaf directories, 128 empty directories and 4,096 files of 1–32 bytes; expected 67,584 bytes, 264 descendant folders, 265 directory results.

The example asserts totals, descendant result counts, zero errors/skips and completion on every run. Fixture creation is separate from scan measurements. The first recorded baseline was taken before scanner edits. The final comparison uses saved baseline/final executables in ABBA order, after Cargo compilation finishes, on the same unchanged fixtures. Each process warms each workload once, then records seven scans of both raw traversal and the unchanged automatic wrapper with one admitted root/worker. This yields 14 measured samples per version/workload/mode.

These are **unoptimized debug builds on warm local filesystem caches**, with the same counting allocator. They establish this experiment's improvement, not Release-build ratios or cold HDD/SMB performance. The automatic measurements include existing policy checks, background priority and pacing. Wall time uses `Instant`; process CPU uses Windows `GetProcessTimes`. CPU accounting is coarse (observed 15.625 ms increments), so the CPU column uses the mean over the measured scans rather than implying fine-grained individual CPU timing.

Allocation counts come from the example's global allocator. Peak memory is the maximum additional **requested Rust heap bytes** during a scan, not process RSS, native/kernel memory or total allocated bytes. Windows `GetProcessIoCounters` separately records whole-benchmark-process “other I/O operations.” That is an observed OS counter, **not an exact metadata-call trace or physical disk-read count**.

## Performance results

Final alternating-run results are recorded in the accompanying `summary.json` and `quiet-process-io.json`. Times below are milliseconds; heap values are requested bytes.

| Workload / mode | Wall median before → after | Mean process CPU before → after | Allocation calls before → after | Peak additional Rust heap before → after |
| --- | --- | --- | --- | --- |
| Small / raw | 325.52 → 14.39 | 320.31 → 15.63 | 132,055 → 132,059 | 5,685 → 185,480 |
| Small / automatic | 852.04 → 250.03 | 665.18 → 185.27 | 1,272,198 → 1,272,202 | 6,050 → 185,680 |
| Nested / raw | 156.55 → 19.08 | 155.13 → 17.86 | 52,239 → 51,316 | 6,418 → 79,148 |
| Nested / automatic | 352.75 → 129.70 | 266.74 → 114.96 | 543,766 → 542,843 | 6,618 → 79,348 |

For the unchanged automatic wrapper this is about **71% lower wall time / 72% lower process CPU** on the flat fixture, and **63% lower wall time / 57% lower process CPU** on the nested fixture. Flat allocation counts are effectively unchanged (+4); nested counts decrease modestly. There is no claim of a general allocation reduction. Peak requested Rust heap increases by roughly 175 KiB (flat) or 71 KiB (nested), from the shared scratch page and bounded decoded queues. Raw-traversal wall ranges were 322.14–335.49 → 13.89–15.51 ms (flat), and 145.04–169.78 → 18.71–19.87 ms (nested). Automatic wall ranges were 827.16–865.45 → 245.42–252.49 ms and 334.39–359.90 → 125.86–148.93 ms respectively.

Whole-process Windows “other I/O operation” counts for the two baseline executions were **1,073,850 and 1,073,848**, versus **22,218 and 22,216** for the candidate executions. Each execution includes all four modes/workloads, their warm-ups, startup and output, so this is a corroborating counter rather than per-scan metadata-call attribution. Read-operation/byte counters also include process setup; they are not a scanner-content-read benchmark.

The optimization trades a bounded enumeration-page working set for fewer filesystem metadata requests. It is not a general memory-reduction change. Scheduling and cache capacity were not increased to produce the timing result.

Explicit source-level metadata call counts for one successful supported-native scan:

| Fixture | Before: `symlink_metadata` | After: `symlink_metadata` | After: directory-handle attribute queries |
| --- | ---: | ---: | ---: |
| Small | 12,002 | 2 | 1 |
| Nested | 4,362 | 2 | 265 |

These counts are derived from the scanner paths and fixture cardinalities, not instrumented syscall traces. They exclude canonicalization and operations internal to enumeration/standard-library calls. Portable fallback can have additional first-attempt and safety-check overhead; it is not covered by the supported-NTFS performance claim.

Combining these explicit calls gives 12,002 → 3 (flat) and 4,362 → 267 (nested). The eliminated calls are the old per-child pathname metadata requests; a per-opened-directory handle check is deliberately retained for the replacement race. Exact OS metadata syscall counts were not separately traced.

## Correctness and regression results

| Command, with `--offline --locked -j 4` | Result |
| --- | --- |
| `cargo test -p atlas-core --lib fs::folder_size::tests` | 15 passed, 1 prerequisite-dependent symlink case ignored |
| `cargo test -p atlas-core --test folder_statistics --test group47_persistence --test group48` | 43 + 2 + 3 passed |
| `cargo test -p atlas-model --lib folder_stats` | 19 passed |
| `cargo test -p atlas-win-space --lib directory::tests` | 11 passed |
| `cargo check -p atlas-ui --lib` | Passed; dev profile, 45.85 s, 163 warnings, no errors |
| `cargo check -p atlas --bin FileOP` | Passed; dev profile, 29.56 s; check only |

Total passing focused test executions: **93**. The existing statistics suite includes the literal `≥` presentation assertions, descendant reuse, incomplete/cancelled results, persistence, visible-cache protection, automatic cancellation, media admission and eviction/rescan-loop regressions. No indicator or cache implementation was edited.

Added tests cover multi-page native/portable metadata equivalence with 2,048 files; cancellation inside a page; inaccessible directory contents using a temporary directory-only deny-list ACL restored before assertions; directory deletion after enumeration; directory replacement with a junction before opening; cached sizes during mutation and fresh values on a subsequent scan; real offline attributes; synthetic recall/reparse flags; and the original depth limit. Existing tests cover ordinary nested/empty directories, overlaps and junction cycles. Native parser/malformed-buffer and listing-equivalence tests also pass.

The first directory-replacement test found that a no-follow junction handle can enumerate as empty. The opened-handle attribute check was added and that regression now passes. Partial native pages are validated before exposure; parser failures do not leak partially decoded results.

The explicit file/directory/broken-symlink test was attempted separately and failed at fixture creation with Windows error **1314, ERROR_PRIVILEGE_NOT_HELD**. It could not exercise scanner runtime behavior here. No privilege or Developer Mode configuration was changed. Real junction behavior and the shared native exclusion bits were tested, but they do not substitute for that physical symlink case. Actual provider-managed cloud placeholders were not provisioned; real offline and synthetic recall/reparse attribute coverage are reported separately.

## Evidence and remaining acceptance

Audit directory: `%TEMP%\fileop-stats-20261117-084434`. It retains `folder_size-before.rs`, the baseline/final benchmark executables, original `before.csv`, alternating `quiet-*.csv`, `summary.json`, `quiet-process-io.json`, build/test/check logs and the failed symlink prerequisite log. `compare.ps1` performs the alternating comparison. The preservation snapshot was taken after adding the benchmark but before editing production scanner/native source; `preservation.json` identifies only those two existing source files as changed.

Physical tests recommended, using disposable fixtures only:

- In FileOP, observe automatic Details/List/Grid calculations and manual totals for the same tree. Verify Files/Folders/size agreement, descendant reuse, partial `≥` values, selection/navigation continuity and no new scans after idle or view changes.
- Compare warm and cold cohorts on SSD, HDD, removable FAT/exFAT and SMB, including a filesystem/provider that rejects the native information class. Verify fallback totals and partial failures before comparing speed.
- With existing symlink privilege/Developer Mode, run the ignored file/directory/broken-link test. Also check real provider cloud placeholders without allowing hydration, and reparse/mount-point fixtures.
- Exercise permission denial, disconnect during enumeration and cancellation during a slow directory query. A synchronous provider/kernel call cannot be forcibly interrupted by the existing cancellation flag; checks resume between entries/pages when it returns.
- Mutate and rename trees during scans, including directory replacement, and verify explicit incomplete states and refresh on subsequent scans. Final-component replacement is tested; no transactional snapshot or new general ancestor-race resistance is claimed.
- If production performance ratios are needed, measure an independently approved optimized candidate later. No full Release build was performed for this task.

No cache eviction/capacity/invalidation/persistence, worker counts, storage policy, background priority, UI, transfer execution, JPEG decoding, search indexing or Copy Manager source was changed. No existing releases or conflicted copies were touched. Untracked `FLDR-icon.png` and `FLDR.psd` appeared during the task; those unrelated artwork files were left untouched.
