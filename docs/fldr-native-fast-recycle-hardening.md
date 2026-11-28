# Native Fast Recycle hardening — 2026-11-19

## Follow-up: specific-item recognition correction

The user reported that the hardened move succeeded in 478 microseconds, specific
verification failed in 8,904 microseconds, rollback succeeded, and the whole
operation reported `native_error elapsed_ms=19`. Shell deletion with the opt-in
variable set to `0` was independently retested successfully by the user.

This follow-up changes only:

- `crates/atlas-ui/src/fast_recycle.rs`
- `crates/atlas-ui/src/fast_recycle_tests.rs`
- `docs/fldr-native-fast-recycle-hardening.md`

### Reproduced defect and correction

The old hardened implementation held `$I` open with GENERIC_READ, GENERIC_WRITE
and DELETE access during Shell recognition. Setting FILE_SHARE_READ on that handle
was insufficient: sharing compatibility is bidirectional. A subsequent reader
that denies writing/deletion cannot coexist with the retained writer/DELETE
access. The earlier implementation closed its metadata writer before verification.

A new non-destructive regression uses Windows' real `SHCreateStreamOnFileEx` with
`STGM_READ | STGM_SHARE_DENY_WRITE` on a private Temp record. It reproduces
`ERROR_SHARING_VIOLATION` (HRESULT `0x80070020`) with the old handle and successfully
reads the exact bytes after the correction. This establishes the sharing defect
with an actual Shell stream reader; it does not independently prove which internal
reader mode the Recycle Bin namespace used during the reported live failure.

`metadata_read_guard` now uses `ReOpenFile` to retain an identity anchor to the
same metadata object, closes the writer/DELETE handle, and obtains a read-only
guard. The guard permits read-only consumers while denying writes, deletion and
rename. ReOpenFile deliberately replaces the file object; DuplicateHandle would
retain the original file object's sharing state. The metadata's file ID,
reserved pathname and safe attributes are checked after the handoff, before
moving the folder. Reopen calls use OPEN_REPARSE_POINT.
The brief handoff window cannot cause pathname-based cleanup of an unrelated file:
cleanup reopens the retained object for DELETE access, never the pathname.

The selected folder's DELETE-capable handle and ancestor pins remain unchanged,
preserving rollback by the original object. If verification fails, rollback still
runs first. Cleanup then releases only the metadata read guard and disposes of the
owned metadata object. Failed rollback retains the metadata/payload recovery pair;
failed cleanup remains an explicit error. Specific-item verification is not bypassed.

### Enumeration, timing and notification findings

Read-only Shell automation outside the restricted tool context returned 21 items,
all with physical recycled paths. The restricted context returned zero items. No
item names or original paths were printed and no item was modified. This explains
why a zero-count automated diagnostic alone is not evidence of invalid metadata.
It does not establish the context of the user's running application.

The existing `IShellFolder2` enumeration, exact target matching and original-name
check are retained. Notification order is unchanged: no delay, polling loop or
early success notification was added. There is no measured evidence requiring a
notification-timing workaround. The correction removes the demonstrated sharing
barrier before requesting a fresh namespace enumeration.

Trace records now separate `metadata_reader_handoff`, `pair_verification`, and
`shell_namespace_verification` from the enclosing verification stage. Rejection
messages report enumerated count, filesystem-path count, and whether the exact
target was found, without disclosing unrelated paths. Empty enumeration and an
original-name mismatch can therefore be distinguished on the next physical test.

### Follow-up validation and remaining blocker

**34 automated tests passed**, zero failed; the historical component benchmark
remains ignored in the ordinary suite and was not rerun for this correctness fix.
Nine new tests cover the Shell stream sharing regression, retained read protection,
identity-safe cleanup after a path replacement, wrong reserved-path rejection,
rollback/cleanup with the new guard, rollback collision preservation, Shell
filesystem binding while the payload handle remains retained, cancellation before
move/after move, and explicit retryable cleanup failure.

Offline locked FLDR entrypoint check succeeded in **9.54 seconds** using Rust
`+1.97.1` and four jobs. The normal debug executable was locked; that build failed
to replace it, and no process was stopped. A separate debug-equivalent candidate
**built successfully** under `C:\Build\FileOP\recognition-check\FLDR.exe` using a
session-only profile inheriting `dev`; all build outputs remain under
`C:\Build\FileOP`. The fresh profile's dependency compilation and link took
**15 minutes 15 seconds**; this is a build duration, not a recycle benchmark.
The final no-reparse source then passed all 34 automated tests again.

Candidate size: **98,256,896 bytes**. SHA-256:
`D7C7F8A00938993163BA47EB4241DD94B2C8F8D21FE3C7B0A85B32B05C560114`.

Reproduce the isolated build in the configured MSVC/SDK environment:

```powershell
$env:CARGO_TARGET_DIR = 'C:\Build\FileOP'
$profileFile = Join-Path $env:TEMP 'fldr-recognition-profile.toml'
Set-Content -LiteralPath $profileFile -Value @('[profile.recognition-check]', "inherits = 'dev'")
cargo +1.97.1 build -p atlas --bin FLDR --offline --locked -j 4 --profile recognition-check --config $profileFile
```

No repository or persistent Cargo profile configuration was changed. Follow-up
logs are in Temp: `fldr-recycle-recognition-check.log`,
`fldr-recycle-recognition-candidate-build.log`, and
`fldr-fast-recycle-recognition-tests.log`.

No live Recycle Bin deletion or restore was performed for this correction. All
mutation tests use new private Temp directories. No UI, dispatch, copy/move,
scanner, thumbnail, settings or registration source changed. Native recycling
still requires `FLDR_FAST_RECYCLE=1`.

The next approved integration test must use the **new candidate**, not the locked
older debug executable: create a fresh ten-file disposable payload and external
hash manifest using the procedure below, Delete once in the candidate, require
`native_success`, verify that Explorer lists that exact item, Restore only that
item, then compare all hashes. Collect the new pair/namespace/handoff stages.
Use the candidate path specified above in place of `debug\FLDR.exe` in that
procedure. Do not silently reuse a running older FLDR instance.

Full Recycle Bin recognition/Restore with the corrected binary remains a live
integration blocker. The private Shell stream regression is not a replacement
for it. Default activation remains unjustified; no new end-to-end speed claim is
made from the earlier component benchmark or the user's rollback timing.

Primary API references: [CreateFile sharing rules](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew),
[ReOpenFile object/access semantics](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-reopenfile),
[SHCreateStreamOnFileEx](https://learn.microsoft.com/en-us/windows/win32/api/shlwapi/nf-shlwapi-shcreatestreamonfileex).

The remainder of this document records the initial hardening batch; its retained
metadata writer description is superseded by the reader handoff above.

## Scope and status

Implemented in the current working tree, preserving existing uncommitted work.
Native recycling still requires `FLDR_FAST_RECYCLE=1`. Windows Shell remains the
default and handles pre-mutation unsupported/unsafe cases. No real Recycle Bin
mutation, application launch, Release build, installation, or Git mutation was
performed for this implementation batch.

The user confirmed that Windows recognized the earlier native test and restored
all ten files with correct contents. That is evidence for the earlier format,
not runtime qualification of the newly corrected implementation.

## Exact files changed in this batch

- `crates/atlas-ui/src/fast_recycle.rs`
- `crates/atlas-ui/src/fast_recycle_tests.rs` (new)
- `crates/atlas-ui/src/shell_file_operation.rs` (native delete dispatch only)
- `scripts/test-fast-recycle.ps1` (new isolated source-module test harness)
- `docs/fldr-native-fast-recycle-hardening.md` (this report)

No UI, scanner/cache, thumbnails, listing, copy/move, or Undo implementation changed.

## Specific-item verification and compatibility

The format remains version 2: 64-bit version, logical bytes and FILETIME,
32-bit UTF-16 code-unit count including NUL, then the exact UTF-16 path and NUL.
No lossy conversion is used when encoding metadata or retaining extensions.
Native eligibility conservatively declines non-Unicode and extended/device paths;
metadata unit tests independently verify preservation of unpaired UTF-16.

Success now requires all of:

1. Re-read this operation's retained metadata handle; length and bytes must equal
   the complete expected record.
2. Open the exact `$R` destination without following a reparse point. Its volume
   serial/file ID must match the retained source handle's pre-move identity.
3. Confirm that the original path is absent, treating any unexpected probe error
   as a verification failure.
4. Enumerate Windows' real Recycle Bin namespace with `IShellFolder2`, stopping
   when this exact physical `$R` path is found, and check its original folder
   display name. An unrelated count increase cannot satisfy this check.

Shell enumeration failure, lack of recognition, or cancellation causes rollback.
This is a compatibility check, not proof that a future Restore will succeed:
later path conflicts, permissions, external mutations and crashes remain possible.
In particular, the new retained-handle sharing and Shell recognition combination
requires a controlled real-bin integration test before default activation.

## Mutation, rollback, cleanup and cancellation

- Pin source ancestors and bin ancestors with no delete sharing. Directory-list
  access is intentional: attribute-only handles did not reliably block rename.
- Keep the selected source open with DELETE access. Move and rollback use
  `SetFileInformationByHandle(FileRenameInfo)` on that exact object, without
  replacement or cross-volume copying. The filename buffer is NUL-terminated.
- Reserve `$I` with exclusive creation. Keep its owned handle without write/delete
  sharing. Cleanup marks that exact object delete-pending by handle, so a path
  replacement cannot make cleanup delete an unrelated metadata file.
- Metadata write/flush failure attempts owned cleanup and reports failure.
- If a move fails and the retained source identity is confirmed still at its
  original path, clean metadata and report the move error. Ambiguous failures
  retain metadata and expose original/payload/metadata locations for recovery.
- Failed verification rolls back by the retained source handle, checks restored
  source identity, then cleans metadata. No Shell fallback occurs after a move
  attempt, including successful rollback.
- Failed rollback or uncertain rollback verification preserves recovery metadata;
  errors include recovery locations. Cleanup failures are explicit errors.
- Cancellation is checked at entry, during traversal, during reservation, before
  the move, during Shell enumeration, and before commit. Cancellation after a
  move attempts rollback. Once verified commit completes, later cancellation does
  not rewrite the completed item's outcome.
- Delete dispatch reports cancellation for every unvisited selection entry and
  retains already completed items. Existing completion/error refresh remains in
  place; `OpComplete` refreshes affected tabs even on error.

There is no crash-recovery service or automatic bin cleanup. A process crash can
leave prepared metadata without a payload; power-loss durability has not been
tested. Such remnants must not be automatically removed from the live bin.

## Eligibility and traversal safety

Drive-relative, UNC, extended/device, root-only, dot-component, alternate-stream,
trailing-dot/space and ambiguous separator paths use Shell. Protected `$Recycle.Bin`
and `System Volume Information` component comparisons are case-insensitive.
Drive letters are normalized for the generated destination path.

Selected folders, source/bin ancestors, and traversed children reject reparse,
offline and recall attributes. Junction/mount/symlink paths therefore use Shell.
Traversal retains pinned directories, rejecting redirection without walking
through it. A 4,096-directory pin budget avoids unbounded handle consumption;
larger directory trees use Shell before mutation. Files remain free to change
while scanning, so size/quota accounting is advisory rather than a snapshot.
Unreadable or incorrectly typed policy values now decline native operation;
they are not interpreted as permissive missing settings.

## Performance findings and diagnostics

Removed the full-bin `$I` format walk. It did not establish validity of a newly
created item and performed work proportional to unrelated bin contents. Removed
the second aggregate query used for success verification. One aggregate query
remains solely for capacity estimation; exact Shell recognition can still involve
a namespace walk. No background work, cached global state or worker was added.

Private synthetic fixture, Windows debug build, 536 metadata records, 31 warm runs:

| Component | Old | Corrected |
|---|---:|---:|
| Full-bin format walk | 26.545 ms median | Not performed |
| Specific metadata/payload verification | Absent | 0.078 ms median |
| Metadata data reads in those components | 536 | 1 |

These are separate component measurements, not interchangeable verification
algorithms and not end-to-end recycle timings. Shell recognition, quota queries,
metadata flushes and notifications are excluded. No end-to-end speedup over Shell
is established, and the historical 1,960 ms delay remains unattributed.

With `FLDR_FAST_RECYCLE_TRACE=1`, the existing Temp trace gains path-free
`stage=... elapsed_us=... thread=...` records for eligibility, configuration,
capacity query, size scan, metadata preparation, metadata write/flush, handle rename,
specific-item verification, rollback/cleanup and notifications. The original
`event=native_success|shell_fallback|native_error elapsed_ms=...` record remains,
with `native_cancelled` added. Rollback handle renames have their own rename record
inside rollback timing. Stage records can nest and must not all be summed.
Tracing itself adds file I/O when explicitly enabled.

## Automated validation

Isolated harness compiles the actual production source and its tests with the
same Windows crate version, outside the synced project. It never invokes the
live-bin executor. Fixtures are newly created private Temp directories; tests
use private payload and metadata names, never the real `$Recycle.Bin`.

Commands (PowerShell, Cargo on PATH):

```powershell
$env:PATH = 'D:\DevTools\Cargo\bin;' + $env:PATH
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\test-fast-recycle.ps1
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\test-fast-recycle.ps1 -Benchmark
```

Results: **25 tests passed**, zero failed; one component benchmark is ignored by
default and **passed separately**. Coverage includes UTF-16 format, path guards,
suffix mixing, exclusive reservation, nested totals, cancellation, directory pin
enforcement, actual retained-handle movement, corrupt metadata, wrong payload ID,
no-replace collisions, write failure, ambiguous move failure, known failed move
cleanup, actual verification rollback, rollback collision preserving unrelated
data, cleanup failure, junction/ancestor rejection and offline attributes.

The first test run exposed attribute-only pin sharing and missing rename NUL
termination; both were corrected and the relevant real-filesystem tests pass.
The offline-attribute fixture is not a real cloud provider placeholder test.

Offline locked entrypoint check and debug build use Rust `+1.97.1`, `-j 4` and
`CARGO_TARGET_DIR=C:\Build\FileOP`. Output: `C:\Build\FileOP\debug\FLDR.exe`.
Final command results: both succeeded, respectively **10.51 s** and **26.32 s**
on the final production source. Existing workspace warnings remain (164 UI
warnings on the debug build); no warning cleanup was attempted. The executable
is **98,254,336 bytes** and was not launched.

```powershell
$env:CARGO_TARGET_DIR = 'C:\Build\FileOP'
cargo +1.97.1 check -p atlas --bin FLDR --offline --locked -j 4
cargo +1.97.1 build -p atlas --bin FLDR --offline --locked -j 4
```

The build used the machine's existing MSVC 14.51.36231 and Windows SDK
10.0.26100.0 include/resource-tool paths in the build process environment only.
Logs remain in Temp as `fldr-recycle-check.log`, `fldr-recycle-build.log`,
`fldr-fast-recycle-tests.log`, and `fldr-fast-recycle-benchmark.log`.

## Required controlled integration before default activation

These steps require user approval before any live-bin deletion. They were **not
run automatically**. Close older FLDR processes manually first so a previous
instance does not retain another executable or environment.

Prepare a new, isolated fixture and hash manifest (PowerShell):

```powershell
$caseRoot = Join-Path 'C:\Build\FileOP\experiments' ('recycle-it-' + [guid]::NewGuid().ToString('N'))
if (Test-Path -LiteralPath $caseRoot) { throw 'Fixture path already exists' }
$payload = Join-Path $caseRoot 'payload'
New-Item -ItemType Directory -Path $payload | Out-Null
foreach ($n in 1..10) {
    [IO.File]::WriteAllText((Join-Path $payload "test-$n.txt"), "FLDR validation file $n`r`n", [Text.UTF8Encoding]::new($false))
}
$expected = @(Get-ChildItem -LiteralPath $payload -File | ForEach-Object {
    [pscustomobject]@{Name=$_.Name; Length=$_.Length; SHA256=(Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash}
})
$expected | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $caseRoot 'expected.json')
$env:FLDR_FAST_RECYCLE = '1'
$env:FLDR_FAST_RECYCLE_TRACE = '1'
$trace = Join-Path $env:TEMP 'FLDR-Fast-Recycle-Trace.log'
$traceStart = if (Test-Path -LiteralPath $trace) { @(Get-Content -LiteralPath $trace).Count } else { 0 }
Start-Process -FilePath 'C:\Build\FileOP\debug\FLDR.exe'
```

In that debug FLDR instance, navigate to `$caseRoot`, select only `payload` and
press Delete. Require a new `native_success` record, no Shell fallback, correct
directory refresh, and the exact payload visible in Explorer Recycle Bin. Use
Explorer Restore on **only that entry**, then verify:

```powershell
if (-not (Test-Path -LiteralPath $payload)) { throw 'Restore did not return the expected folder' }
$expected = @(Get-Content -LiteralPath (Join-Path $caseRoot 'expected.json') -Raw | ConvertFrom-Json)
$actual = @(Get-ChildItem -LiteralPath $payload -File | ForEach-Object {
    [pscustomobject]@{Name=$_.Name; Length=$_.Length; SHA256=(Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash}
})
$difference = Compare-Object $expected $actual -Property Name,Length,SHA256
if ($difference) { $difference; throw 'Restored contents differ' }
Get-Content -LiteralPath $trace | Select-Object -Skip $traceStart
```

Then use fresh isolated fixtures for each case; keep manifests outside payloads:

| Case | Exact action and acceptance |
|---|---|
| Empty, dotted and Unicode folders | Delete and restore each; exact original name/location, no missing children. |
| 20,000 files and nested directories | Save relative-path/hash manifests; require native success where eligible; restore and compare every entry. More than 4,096 directories must decline native operation. |
| Multiple selections | Mix folders and files; completed subset and cancellations must match actual results. Native folders and Shell-only files retain correct per-item outcomes. |
| Junction, symlink, mount ancestor | Create links only between disposable locations; select linked/containing folders; require Shell fallback. Restore and confirm linked targets' hashes never changed. |
| Real cloud placeholders | Use provider-approved disposable fixtures; require fallback without native hydration. Offline attribute unit tests alone do not qualify this. |
| Permissions / unreadable children | Use a disposable restricted test account or VM fixture; require safe Shell/elevation handling or an explicit error, never native success. |
| Cancellation | Cancel during a large scan and during verification; require restored original after rollback or a completed verified item, with accurate selection-wide reporting. |
| Restore destination conflict | Recreate the original name after successful recycling; Explorer must present its normal conflict handling. Never test against user data. |
| Concurrent operations | Recycle distinct disposable folders with identical names from different parents; restore each and verify its own manifest. Unit tests separately cover forced name collisions. |
| Performance | Compare at least five fresh identical fixtures per route (Shell with variable absent, native with `1`), same volume and cache conditions; require route traces and restore/hash checks for every run. Compare medians and slowest runs; repeat with trace disabled for unbiased latency. |
| Navigation/startup | Compare idle CPU, startup and repeated browsing with the variable absent versus `1`, without initiating deletion; do not infer causality from one subjective run. |
| Crash/power loss | Disposable VM/snapshot only, with explicit approval and recovery planning; inspect prepared metadata/payload states without automatic cleanup. |

Forced rollback and cleanup failures are covered by private-directory unit tests;
do not inject corrupt metadata or forced collisions into the live bin.
To return to default Shell behavior, remove `FLDR_FAST_RECYCLE` from the launching
PowerShell environment and start a fresh FLDR process after closing the opt-in
instance. No persistent setting or registry switch was introduced.

## Release checklist

- PASS: automated identity, metadata, collision, rollback and cancellation tests.
- PASS: no change to default Shell selection; native remains opt-in.
- PENDING: corrected binary's real-bin recognition, Restore and integrity.
- PENDING: real symlink/cloud/permissions and mixed-selection integration.
- PENDING: end-to-end advantage over Shell and measured stage attribution.
- PENDING: controlled startup/navigation comparison and crash qualification.

**Default activation is not recommended until the pending criteria pass.**
