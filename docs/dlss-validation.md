# DLSS remediation: validation record

Updated 1 November 2026. Original aggregate code under test: `dbde495af566192b488df99937aa6956248e22d3`,
compared with the original fork base `763528025d89cf575d7335e2a78d7fc48397d449`.
This record distinguishes implemented behavior, automated evidence and hardware
acceptance. The complete integration is **not yet accepted for release**.

The repeat full review of `8b33c7954` found F1–F5. F1's DX12 creation and
lifetime fix passed fresh independent `fix_f1_review2` on `aad4e963f`.
F2's gameplay/screen/loading gate passed fresh independent `fix_f2_review2` on
`534b9ff4a`. F3's report microseconds fix passed fresh independent
`fix_f3_review` on `b15183c53`. F4's entity motion registration and GPU
regressions passed fresh independent `fix_f4_review` on `a7b0d7907` (including
`934ebb35b`). F5 startup diagnostics are implemented and await fresh independent
review. F1–F4 are accepted; F5 and hardware acceptance remain open. Earlier per-part
PASS results do not close the repeat review.

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

F2's semantic menu suspension change is based on F1's accepted `aad4e963f`.
The renderer consumes `Frame.framegen_allowed`, set only for Playing with a
player and no screen overlay, pause or local/remote loading. Every GUI screen
and every loading frame disables FG regardless of visual blur. HUD, tab list
and portal effects remain gameplay until a GUI or loading state opens. Existing
title, focus, HDR, shader, presentation, input and history-reset gates remain.

Targeted Windows checks passed for F2:

```powershell
cargo test --locked -p horizon-client --bin horizon-client framegen_ -- --test-threads=1
cargo test --locked -p horizon-streamline --lib -- --test-threads=1
cargo test --locked -p horizon-streamline --lib failed_live_tags_or_options -- --ignored --test-threads=1 --nocapture
cargo test --locked -p horizon-client-render --lib gfx::frame_gen::lifecycle_tests -- --test-threads=1
cargo clippy --locked -p horizon-client-render -p horizon-streamline -p horizon-client --lib --bins --tests -- -D warnings
```

Results: 2 policy/loading-frame tests; 21 ordinary Streamline tests with 7
explicitly ignored; 1 explicit GPU recording test on both Vulkan and DX12;
6 renderer lifecycle/history tests. Policy covers pause/Options with blur 0/5,
hidden pause menu, inventory/chat/death, every other Screen, loading/world
transition, paused simulation, screens over Playing and gameplay resume.
The GPU test uses production Session::record and submit with mock SDK calls:
three blocked frames retain wanted/reset, report actual=0, clear all tags and
send Off once; eligible resume sends live tags/On and consumes reset. Existing
history tests confirm blocked frames do not repeatedly reset fallback TAA.
Formatting, public-tree and diff checks also passed. No game was launched;
focused FG/extra displayed frames, Linux/macOS and full pr_ready were not run.
Fresh independent `fix_f2_review2` accepted the follow-up below; these tests do
not close hardware acceptance or F3–F5.

Fresh review rejected F2 commit `4126aeb50`: its loading predicate reused the
visual panorama helper, which excludes Nether/End screens. PlayerPosition can
restore the player before LevelLoadTracker finishes, allowing FG during portal
loading. The follow-up uses a separate semantic remote loading predicate: any
active tracker, connecting without a player or reconfiguration suspends FG.
Visual panorama/blur policy is unchanged. The targeted client command above now
passes 3 tests: the new regression feeds a real LevelLoadTracker through
Other/NetherPortal/EndPortal, with a restored player still waiting for server,
player chunk, ready/close delay and screen finish, then gameplay resume. The
same predicate is used by the production Frame producer. Targeted client/render/
Streamline lib/bins/tests Clippy, formatting, public-tree and diff checks passed
again on this follow-up. Fresh independent `fix_f2_review2` accepted
`534b9ff4a` (PASS).

F3 removes the QPF conversion from `Session::read_reports`: report timestamps
are already microseconds, so `latency_us` is their difference. The unused
QueryPerformanceFrequency declaration, constructor call and qpf field are gone.
Input→GPU priority, simulation→present fallback, newest valid frame selection
and existing overlay labels are unchanged.

Primary contract evidence, with the root MIT license and each source header
checked before reading, is the pinned Streamline commit
`625db07d4bca5abc67d781c42af7e7fb0761f1b6`:

- [`reflexEntry.cpp`](https://github.com/NVIDIA-RTX/Streamline/blob/625db07d4bca5abc67d781c42af7e7fb0761f1b6/source/plugins/sl.reflex/reflexEntry.cpp) delegates to the compute backend's latency report query.
- [`generic.cpp`](https://github.com/NVIDIA-RTX/Streamline/blob/625db07d4bca5abc67d781c42af7e7fb0761f1b6/source/platforms/sl.chi/generic.cpp) copies NVAPI report timestamps without scaling.
- [`nvllvk.cpp`](https://github.com/NVIDIA-RTX/Streamline/blob/625db07d4bca5abc67d781c42af7e7fb0761f1b6/source/platforms/sl.chi/nvllvk.cpp) and [`vknvll2.cpp`](https://github.com/NVIDIA-RTX/Streamline/blob/625db07d4bca5abc67d781c42af7e7fb0761f1b6/source/platforms/sl.chi/vknvll2.cpp) copy native timestamps unchanged; the latter copies Vulkan `*TimeUs` fields.
- [Khronos's VkLatencyTimingsFrameReportNV reference](https://docs.vulkan.org/refpages/latest/refpages/source/VkLatencyTimingsFrameReportNV.html) defines those microsecond timestamp fields.

No SDK implementation code was copied. Four new ordinary tests write SDK-shaped
reports into the production Session buffer and call `read_reports`. The known
20,000 us difference failed before the fix (2,000 us with QPF explicitly set to
10 MHz). It now produces exactly `Reflex: 20.0 ms`, including near u64::MAX.
Other cases cover input→GPU priority, missing-input/GPU fallback, unordered
frames with invalid newer entries, clearing stale latency and a maximal valid
difference without multiplication overflow. The final tests and constructor
have no QPF or CPU-clock assumption. Existing availability/error tests still
clear latency when reports or low latency become unavailable.

F3 checks passed: `cargo test --locked -p horizon-streamline --lib --
--test-threads=1` (**25 passed, 7 ignored**), `cargo clippy --locked -p
horizon-streamline --lib --tests -- -D warnings`, workspace formatting,
public-tree and diff checks. Fresh independent `fix_f3_review` accepted
`b15183c53` (PASS). No game,
SDK or GPU test was started for this units fix; full pr_ready, Linux/macOS and
physical latency were not rerun. The historical 9.4 ms overlay in the earlier
windowed attempt used the incorrect conversion and cannot establish corrected
latency. It has not been retrospectively rescaled or used as acceptance proof.

F4's production change `934ebb35b` replaces the item/mob boolean namespace
with typed renderer/entity/block keys. Known entity UUIDs distinguish reused
network IDs; locally allocated IDs are monotonic within a level. Per-object
ranges are registered after existing culling and before material sorting.
The renderer still reads the original `Frame.entities` offsets. Dragon geometry
now appends directly to that buffer, so its spans use final offsets too.

Registration covers boats/carts, local third-person and LAN guest players
(remote players already use the mob path), falling blocks, TNT, armor stands,
projectiles/tridents/rockets/potions/fangs, block/item displays, crystals/eyes,
dragons/fireballs, leashes/hooks, frame bodies/items/maps, paintings/cushions
and the elder apparition. Animated block entity meshes (including chest lids,
shulkers and banners), moving pistons, spawner previews, vault displays,
brushable items and ominous spawners also receive separate stable ranges.
Static campfire/shelf items, transient block replacements and leash knots do
not animate their geometry; their existing camera motion remains sufficient.

Two new CPU tests cover renderer namespace isolation, UUID replacement between
adjacent frames, changed vertex counts and disappearance/reappearance. Existing
UV/layer compatibility, animated geometry, reordering, reset and distant-camera
precision checks remain. New/hidden/replaced/incompatible geometry uses camera
motion until compatible rendered history exists.

```powershell
cargo test --locked -p horizon-client-render --lib gfx::motion -- --test-threads=1
cargo test --locked -p horizon-client-render --lib object_motion_ -- --ignored --test-threads=1 --nocapture
cargo test --locked -p horizon-client-render --lib temporal_inputs_ -- --ignored --test-threads=1 --nocapture
cargo clippy --locked -p horizon-client-render -p horizon-client --lib --bins --tests -- -D warnings
```

Results: **7 motion/history tests**, **2 object-motion GPU readbacks** and
**2 existing temporal-input readbacks** passed. Both GPU regressions ran on the
RTX 5090, Vulkan 621.01 and DX12 32.0.16.2101. The new readback submits
representative opaque boat/player panels through the exact `Span::entity`
registration used by `Game::mark_motion`, then production `Motion::render`.
With a fixed camera, second-frame object pixels contain the expected normalized
X motion **−0.0625**; background, depth-occluded cart, vertex/texture alpha
cutouts and culled backfaces stay zero. The initial reset frame stays zero.
Targeted client check, Clippy, formatting, public-tree and diff checks passed.

This is entity-renderer coverage, not a claim of complete motion for every
pixel: particles, name tags/text and transparent effects retain their existing
behavior. No particle identity/lifecycle system was added. Texture/material or
topology changes can deliberately select camera fallback. No game, real FG,
visual-quality, full pr_ready, parity oracle or Linux/macOS run was performed
for F4. Fresh independent `fix_f4_review` accepted `a7b0d7907` (PASS); F5 and
hardware acceptance remain open.

F5 keeps startup requests and load failures beside the device, separately from
an optional runtime. SDK/path/optional-plugin errors and the original native
device fallback failure now produce `unavailable (reason)`. Only a function
first selected after it was Off when the device was created requires restart.
Off hides failures; reenable preserves the reason. Resource reload reuses the
same metadata; new devices start fresh. SR no longer infers a startup failure
from the global overlay. Native support/bind reports retain their reason.
Ordinary rendering and retry policy are unchanged.

```powershell
cargo test --locked -p horizon-client-render --lib gfx::dlss::tests:: -- --test-threads=1
cargo test --locked -p horizon-client-render --lib graphics::tests:: -- --test-threads=1
cargo clippy --locked -p horizon-client-render -p horizon-client --lib --bins --tests -- -D warnings
cargo fmt --all -- --check
```

Results: **7 DLSS/status tests**, **8 graphics tests**, targeted Clippy,
formatting, public-tree and diff checks passed. Status tests include empty,
missing and relative SDK directories through the real loader, absent optional
DLL names/dependencies, unsupported API, first enable after Off,
disable/reenable, device fallback reason, reload metadata and a fresh device.
Optional DLL tests use the production file-check boundary with an empty
directory; no fake DLL is created or loaded and no signature check is bypassed.
Support/bind reason precedence and fallback/reload metadata are CPU regressions,
not a new windowed hardware acceptance run. Fresh independent F5 review remains
required. No game, actual FG, full pr_ready, parity oracle or Linux/macOS run
was performed for F5.

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

Fresh review rejected `14789df1f`: dropping all external native owners before
Runtime caused access violation during deferred shutdown, while an independent
D3D12 DLL pin prevented it. The fix keeps HAL's actual D3D12 library Arc directly
in SdkLifetime until after shutdown, native/proxy COM Release and SDK DLL unload.
It does not retain a hook or Runtime. The native regression now runs the full
two-chain/resize scenario in both teardown orders and reinitializes the SDK
after each. Fresh independent `fix_f1_review2` accepted `aad4e963f` (PASS).
The extended native test passed in 19.42 seconds (12 configurations and three
SDK sessions); 21 ordinary tests, targeted renderer/Streamline Clippy, workspace
and explicit HAL formatting and diff checks passed again on the fix.

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
