# DLSS remediation: validation record

Updated 1 November 2026. Code under test: `dbde495af566192b488df99937aa6956248e22d3`,
compared with the original fork base `763528025d89cf575d7335e2a78d7fc48397d449`.
This record distinguishes implemented behavior, automated evidence and hardware
acceptance. The complete integration is **not yet accepted for release**.

The repeat full review of `8b33c7954` found F1–F5. F1 now has a DX12
creation integration and a native SDK regression; its fresh independent review
is pending. Earlier per-part PASS results do not close the repeat review.

## Implementation and independent review

Each author change was followed by a fresh reviewer. Failures were fixed by the
same implementer and reviewed again. PASS below applies to code, not the open
hardware/platform acceptance criteria.

| Part | Accepted commits (including fixes) | Independent result |
| --- | --- | --- |
| A — isolated launcher, NVIDIA notices | `1f375ead4` | `review_a` PASS |
| B — non-Windows dependency/stubs | `d1b3a048d` | `review_b` PASS; Linux/macOS execution open |
| C — checked FG Off before surface changes, bounded recovery | `9628c314e`, `49a50de7d`, `c7d2b7a4f`, `5b9677317` | `review_c4` PASS |
| D — every new DX12 swapchain receives a proxy | `98eb2d9b7`, `45e631a73` | `review_d2` PASS |
| E — one frame token and immutable common constants | `f2b07d368` | `review_e` PASS |
| F — retained depth and current motion inputs | `e5a51229a` | `review_f` PASS |
| G — frame-based tags, SDK support, presented-frame counters | `5c3e9ded2`, `8d100b1dd` | `review_g2` PASS |
| H — confirmed capabilities, errors and history recovery | `dd011cbd9`, `112a84fdb`, `dbde495af` | `review_h3` PASS |
| I — final documentation and aggregate validation | This documentation change | Fresh review pending |

The original R1–R9 findings and per-part evidence remain in
[the remediation plan](dlss-remediation-plan.md). R1's code boundary is repaired;
only the Windows target has been executed locally. R2–R6 have code/regression
coverage; their full live FG acceptance is still open. R7/R8 have attribution and
isolation evidence. R9 is addressed by the updated plan, graphics guide and this
record, subject to the final independent review.

## Reproducing isolated Windows checks

F1's explicit regression command is:

```powershell
cargo test --locked -p horizon-streamline --lib official_sdk_dx12_creation -- --ignored --test-threads=1 --nocapture
```

On the hardware below this passed two newly created chains, each resized
800→960→800. It verifies real factory/queue/chain proxies using
`slGetNativeInterface`, acquired HAL textures matching proxy GetBuffer and
differing from native GetBuffer, clear/submit/Present, and complete teardown
after dropping Runtime before the native owners. It runs with FG Off in a
hidden native window: focused FG 2× and additional displayed frames remain
unproven. No focus or signature guard was bypassed.

The same F1 code passed 21 ordinary Streamline tests (7 explicitly ignored),
4 explicit SDK tests including this creation regression and both APIs'
support checks, 4 renderer lifecycle tests, targeted Windows check and
renderer/Streamline lib/tests Clippy with warnings denied. Full pr_ready and
Linux/macOS builds were not repeated for this change. Both real SR pixel
readback tests also passed. DX12 Queue retains its native creating device for
identity checks, since an SDK queue's GetDevice returns its proxy device.

Use a current Rust toolchain, Python 3.10+ and official, compatible NVIDIA
Streamline binaries. The locally tested combination was RTX 5090, driver 621.01,
Streamline 2.14.16.0 / DLSS 310.9.16. SDK headers/license were pinned to commit
`625db07d4bca5abc67d781c42af7e7fb0761f1b6`. SDK/resources/logs stay ignored.
See [SDK dependencies and runtime behavior](graphics.md), including the signed
`NvLowLatencyVk.dll` required by Vulkan Reflex. Do not disable signature checks.

Before any test that can start the game or launcher, create a new scratch:

```powershell
$scratch = Join-Path $PWD ('.local/validation-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory $scratch | Out-Null
foreach ($entry in @(
    @('HORIZON_HOME','home'), @('HORIZON_LAUNCHER_HOME','launcher'),
    @('XDG_DATA_HOME','data'), @('XDG_CONFIG_HOME','config'),
    @('HORIZON_LAUNCHER_TRASH','trash'), @('HORIZON_LAUNCHER_CLIPBOARD','clipboard'),
    @('HORIZON_LAUNCHER_DESKTOP','desktop'), @('TMP','tmp'), @('TEMP','tmp')
)) {
    $path = Join-Path $scratch $entry[1]
    New-Item -ItemType Directory $path -Force | Out-Null
    [Environment]::SetEnvironmentVariable($entry[0], $path, 'Process')
}
$env:HORIZON_LAUNCHER_NO_SELF_UPDATE = '1'
$env:HORIZON_NO_AUDIO = '1'
$env:HORIZON_STREAMLINE_SDK = (Resolve-Path '.local/dlss/native').Path
python tools/pr_ready.py --base 763528025 --quick --verbose
```

Repeat with a fresh scratch for each independent run. Resolve `python`/`cargo`
to the intended installations. Local validation used Python 3.13 and the ignored
`.local/dlss/{cargo,rustup}` toolchain. Restrict timeout cleanup to the PID/tree
started by that check. Never stop other games or reuse player homes.

Targeted commands (GPU tests must run sequentially):

```powershell
cargo test --locked -p horizon-streamline --lib -- --test-threads=1
cargo test --locked -p horizon-streamline --lib official_sdk -- --ignored --test-threads=1
cargo test --locked -p horizon-streamline --lib real_dlss -- --ignored --test-threads=1
cargo test --locked -p horizon-streamline --lib failed_live_tags -- --ignored --test-threads=1
cargo test --locked -p horizon-client-render --lib temporal_inputs_ -- --ignored --test-threads=1
cargo test --locked -p horizon-client-render --lib blocked_fg_preserves -- --ignored --test-threads=1
cargo test --locked -p horizon-client-render --lib gfx::motion -- --test-threads=1
cargo test --locked -p horizon-client-render --lib graphics -- --test-threads=1
python tools/parity.py --changed 763528025 --list
python tools/check_public_tree.py --range 763528025..HEAD
git diff --check
```

## Automated evidence and its limits

The final local self-check used the arguments `--base 763528025 --quick --verbose`
against the code SHA above, with a local supervisor limiting the workspace test
process tree to 180 seconds (300 seconds for other Cargo test steps). Repository
validation tools were not changed. It completed in 5 minutes 10 seconds and
**did not pass**. Its scope also counted the existing untracked `conversation.md`;
that file was neither edited nor staged. Local full logs are `.local/I/*.log`.

| Final check | Result |
| --- | --- |
| Public tree, rustfmt, Windows Clippy | Passed |
| Python suite | 500 run, 2 failures, 2 skipped; baseline comparison below |
| Workspace tests | Timed out at 180 seconds; summary counted 1469 completed passes; not a full workspace pass |
| Client library within that run | 924 passed, 22 ignored; both previously stalled `server_packs::download` tests passed |
| Renderer library, one thread, separate bounded diagnostic | 719 passed, 24 ignored in 33.53 seconds |
| Doc tests | Passed (no active doctests; one ignored) |
| Content-root tests | 6 passed |
| Launcher fmt/Clippy/tests | Passed; 297 passed, 4 ignored |
| Streamline ordinary tests | 21 passed, 6 ignored |
| Explicit official SDK tests | 3 support/requirements + 2 real SR pixel tests passed, sequentially |
| Explicit production error-path GPU test with mock SDK | 1 passed, Vulkan and DX12 |
| Explicit temporal depth/motion GPU readback | 2 passed, Vulkan and DX12 |
| Explicit blocked-FG Post/Motion history GPU regression | 1 passed, Vulkan and DX12, TAA/SR and undersized-output recovery |
| Motion / graphics settings and UI selections | 5 / 25 passed |
| Public history `763528025..HEAD`, `git diff --check` | Passed; no SDK/game files added |

The first sandboxed attempt was stopped after Windows denied process inspection;
the complete retry ran outside that sandbox with fresh homes. The old review's
17 Python failures/4 errors and 27 launcher failures were not reproduced there:
the full current launcher suite passed and Python retained only these two tests:

- `DependencyPathTests.test_traced_dependencies_use_repository_posix_paths`
- `WorkspaceManifestTests.test_a_feature_of_a_workspace_crate_counts_through_its_sources`

Both fail with the same assertions on an archived `763528025` tree and current
code when each uses its own `.local` temporary directory. The relevant
`tools/parity.py` and `tests/test_parity_runner.py` are identical to that base.
The first test patches ROOT while retaining LOCAL, which excludes its fixture
under `.local`; it passed when the archived tree's temp was outside its LOCAL.
The second test's fixture is inside a parent Git checkout, so its `git ls-files`
discovers that checkout instead of falling back to fixture manifests. These
results establish existing fixture/environment failures, not new DLSS code
regressions. They remain failures in the required self-check; no tests were
disabled or altered to make it green.

The full client library's silent-server and invalid URL/hash/size download tests
both passed on retry; the old network stall was not reproduced. The aggregate
run instead stopped in the parallel renderer batch. The same renderer library
passed sequentially; the exact source of the parallel stall is unestablished.
No full-workspace success is inferred from the sequential renderer result.

`pr_ready` listed 359 affected parity harnesses; the direct
`parity.py --changed 763528025 --list` dependency listing reported 367. Neither
list is a parity pass. Escalated inspection confirmed prepared renderer resources
in the installed launcher's version directory, used read-only by the test script,
but no repository `.local/minecraft` Java oracle setup. No oracle comparison was
run. The root lockfile adds only the workspace Streamline dependency and its
existing Rust dependencies; the launcher lockfile is unchanged. Part I changes
only documentation. Linux/macOS execution and the full acceptance matrix below
remain open.

The accepted parts and final targeted reruns establish the following boundaries:

| Evidence | What it establishes | What it does not establish |
| --- | --- | --- |
| SDK support and Reflex On/Boost/Off on Vulkan/DX12 | Actual adapter support, low-latency availability, signed dependency loading | Input feel/latency under a representative load |
| Actual SR pixel readback on Vulkan/DX12 | SDK evaluates SR, frame-tag cleanup succeeds | FG inserts extra displayed frames |
| Headless depth/motion readback on Vulkan/DX12 | Preserved inputs with AA Off/FXAA, OIT/culling combinations, moving geometry | Final windowed FG image quality |
| Injected SDK/queue/surface tests | Off/wait ordering, proxy failure/recovery, bounded retries, constants/tag/options/state failure paths | A native driver survives every live lifecycle transition |
| Real Post/Motion history regression | Blocked FG preserves TAA/SR history; recovery resets once, including below driver minimum | A focused FG Present resumes correctly after every transition |

## Hardware acceptance still required

No row below is a completed FG acceptance result. Record the tested commit, API,
driver/SDK, requested/configured mode, input/output size, real/shown frame counts,
latency availability, scene and logs for each run. Compare equivalent settings
and load; a configured multiplier or increased FPS counter alone is insufficient.

| Scenario, on both Vulkan and DX12 | Remaining proof |
| --- | --- |
| NVIDIA Off; SR/DLAA alone at all qualities; Reflex alone | Ordinary path plus windowed quality, latency and limiter behavior |
| Focused FG 2× without SR, AA Off/FXAA, OIT/culling Off/On | Valid inputs plus actual additional displayed frames |
| SR + Reflex + FG | Same frame token/constants, stable history and real additional presents |
| Resize/fullscreen/minimize/restore/Alt+Tab | Checked Off before mutation, no deadlock, eligible-frame resume |
| Newly created surface after loss | New DX12 proxy before first Present and successful live recovery |
| Title/pause/menu/focus loss/HDR/shader pack | Correct suspension and return with current inputs |
| SDK absent/unsupported/errors, quality/world changes, teleport | Honest fallback and one reset at recovery |
| Long run, light and heavy scenes | No resource growth, acceptable HUD/hand, motion quality and input feel |
| Linux/macOS client and server builds | Actual host/CI results; static cfg inspection is insufficient |

The earlier windowed attempt stayed unfocused and sent cleared tags, so it did
not demonstrate FG. Computer-use focus failed with `GetCursorPos: access denied
(0x80070005)`; UI input stopped and the known test process was closed through
control quit. The SDK's advisory about GetState synchronization remains open,
even though the observed calls/Present shared a thread. No signature or focus
access workaround was used. HUD/hand remain in the generated picture; separate
composition, dynamic generation, RTX shadows and Ray Reconstruction are future
work. Owner visual/input acceptance has not been received.
