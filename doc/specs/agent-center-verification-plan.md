# Agent Center: next verification plan

This plan follows the supplied Agent Center verification report and its B1-B6 findings.
It is a test plan, not a completed product acceptance report.

The current milestone is the continuous responsibility-transfer journey in
[product sections 11 and 12.1](agent-center-product.md), under
[domain specification](agent-center.md) and the
[normative protocol](agent-center-protocol.md). Passing component tests does
not establish that a real coordinator can carry this journey without human
orchestration.

## Scope and explicit exclusions

Verify the confirmed defects: approved criteria, fixed-content acceptance,
coordinator evidence access, cross-work attention, pre-candidate inspection,
guided manual handover, and declined-contract information and disposition.

Do not implement or require the proposed `$` shortcut for opening an ordinary
shell pane in this round. Ordinary shell presentation is separate from managed
work and writer authority.

Manual handback currently conservatively invalidates consumers of the edited
workspace. Record unnecessary re-execution as a known product limitation; do
not claim fine-grained preservation of unaffected same-workspace tasks. Keep
the stronger safety requirement: old evidence must not silently validate
changed content, other workspaces must remain independent, and old artifacts
and history must not be deleted.

Permission/budget expansion, human checkpoint gates, cross-work dependency
editing, external publication, and automatic production repair are not
sign-off targets for these fixes.

## 1. Freeze the build and protect the user's environment

Before a live run, obtain an explicit package selection. Recommend **Dev built
from the repaired feature worktree**; Store is only a shipped-behavior baseline.
Set `ITE2E_PACKAGE=Dev` in every Dev validation process. Do not infer intent
from whichever package or execution alias happens to be installed.

Record the following in the run's evidence:

| Field | Required evidence |
|---|---|
| Source | Branch, HEAD, and a digest or immutable snapshot of the uncommitted changes |
| Executable | Built and deployed `wta.exe` SHA-256; they must match |
| Package | Full package name, family, install location, registration status, and experiment flag |
| Provider | Actual ACP executable/version, selected model, approved destination, authentication readiness |
| Allowance | Explicit planning/execution budgets and authorization to spend real model quota |
| Safety | Existing settings hash, test-owned work IDs/directories/process IDs, cleanup ownership |

Do not terminate the current CLI host, stop unrelated Terminal/WTA processes,
uninstall a package, or replace real project contents. A live authority loaded
from an old binary must be identified before restarting anything; updating the
file alone does not prove that the new service code is running.

Use an isolated, disposable project containing real defect material. Capture a
baseline for its original dirty/staged changes and verify that managed
workspaces do not alter that source checkout.

## 2. Deterministic defect gates

Run the durable Rust regression tests first, then the real ACP/HTTP MCP/native
process conformance tests. For packaged coverage, extend the existing ItE2E
framework rather than inventing another runner.

For each case below, save the exact request/event IDs and before/after state.
An error string alone is not sufficient: assert the downstream state did not
advance incorrectly.

| ID / checklist title | Contract and trigger | Boundary and deterministic oracle | Negative control / existing protection |
|---|---|---|---|
| R1: Agent Center preserves approved initial criteria | Approve a work requiring a concrete check; submit an initial plan changing that requirement to artifact presence or dropping it. | Coordinator request -> plan admission -> work/result/candidate state. Reject the weakening; no applied weak plan, runnable weak dispatch, accepted result, or delivery candidate. | A compatible plan preserving the approved rule can execute and deliver. Preserve same-grant spec revision and stale-version rejection. |
| R2: Agent Center rejects changed delivery content | Produce a candidate through real capture, then change bytes at the same locator before final acceptance. Repeat for File, Tree/GitCommit content, and required check evidence. | Filesystem capture -> persisted references -> human `delivery.accept`. Fail explicitly with the affected reference; no Acceptance and no Completed work. | Unchanged content succeeds; deletion, file/directory substitution, changed manifests and replaced paths also fail. Do not mistake read-only attributes for verification. |
| R3: Agent Center delivers progress findings to coordination | A worker reports a unique finding and requests coordination; let the service create the next coordinator invocation. | Worker MCP -> committed report -> coordinator input/read. The coordinator can obtain the exact finding, reason and referenced evidence without a human copying it. | Reports from another work are denied; duplicate reports do not create duplicate effects; routine progress without a request does not imply human attention. |
| R4: Agent Center exposes captured check diagnostics | A real command gate emits a unique stderr/stdout marker and fails. The coordinator reads the evidence through its supplied tools. | Native process -> immutable diagnostic artifact -> bound HTTP MCP -> coordinator. Assert actual readable bytes or explicit bounded-content metadata, then a rework action bound to the failed result/criterion. | Foreign-work, unavailable or mutated evidence is rejected. Binary/large content must not silently masquerade as complete text; retain valid fixed-input native checks. |
| R5: Agent Center retains cross-work attention | Leave an open decision in A while drafting in B. Refresh B's nonempty inbox, then an empty scoped inbox. | Service response/events -> Console sidebar and work view. A remains visible globally; B's scoped view is accurate; resolving A removes only A's item. | Global refresh may replace the complete global set. Preserve B's text, caret/selection, target and reading position; replayed events do not duplicate attention. |
| R6: Agent Center separates routine progress from notifications | With a pending A decision, send ordinary text/tool/progress events for B and C. | Subscribed work events -> view updates and human-notice surface. Progress updates the appropriate history without replacing the decision notice or stealing focus. | New decision/final-delivery attention remains visible and can update counts; active confirmations remain frozen. Do not merely assert that the event was produced. |
| R7: Agent Center inspects work before a delivery candidate | Open the workspace before any output, then after an intermediate submission but before a candidate. | `workspace.inspect` -> workspace/artifact projection -> Console. Return the real workspace and exact available result references without inventing a candidate or granting human write ownership. | Draft without a provisioned workspace fails clearly; explicit wrong/foreign candidate IDs are rejected. Existing fixed-candidate inspection remains unchanged. |
| R8: Agent Center guides manual takeover and handback | From selected A, request takeover without authored JSON, confirm, edit the managed workspace, and hand back with a summary and explicit continuation choice. | Console preparation -> guarded command -> runtime settlement/capture -> coordinator continuation. Confirm exact Workspace version; wait for TakeoverReady; capture real changed bytes and record the contribution before agent execution resumes. | Switch to B while preparation is pending: never retarget the action. Cancel/stale/error preserves the draft. Full request files are not rewritten. Work-wide Hold is not overridden, and other-workspace tasks remain unchanged. |
| R9: Agent Center preserves declined contract reasons | A real worker declines with a unique explanatory reason, then the adapter ends through cancellation and releases its binding. | Bound acknowledge -> adapter cancellation -> engine terminal classification -> next coordinator. Persist and expose the exact reason; final state/disposition is Failed/ContractDeclined, with reservations released. | Human cancellation remains cancellation; accepted-but-missing-result remains MissingSubmission. Duplicate acknowledgments/observations do not consume extra attempts or erase the original reason. |

The earlier `verification-probes.patch` cases are regression seeds, not a
replacement for real capture/adapter boundary coverage. Keep their failing
intent, but update fixture metadata to the production capture format rather
than retaining success-shaped synthetic receipts.

Suggested local commands:

```powershell
cargo test --target x86_64-pc-windows-msvc --manifest-path tools\wta\Cargo.toml agent_center::
cargo test --target x86_64-pc-windows-msvc --manifest-path tools\wta\Cargo.toml
cargo build --target x86_64-pc-windows-msvc --manifest-path tools\wta\Cargo.toml
```

These are component/regression gates, not packaged UI sign-off.

### Round 2 follow-up gates

Round 2 reached real ACP/model output but stopped at work-MCP initialization.
The client's offered version was not recorded; do not infer it from the test
version below. Round 2 also encountered a fresh-capture publication access
error once, even though subsequent runs passed.

| Gate | Trigger / boundary | Required oracle and negative control |
|---|---|---|
| MCP version negotiation | Offer supported versions, `2025-11-25`, and a future version over real HTTP; then initialize, list bound tools and execute the existing ACP conformance journey. | Known versions are preserved; unknown offers receive the supported `2025-06-18`, not a dropped socket or a claim to support unknown semantics. Missing/non-string/empty versions receive correlated `-32602` errors. Actual work-tool calls, rework and delivery still succeed after negotiation. |
| Capture publication contention | Hold a real Windows descendant handle without delete sharing while publishing its immutable staging directory; release it during the bounded rename retry window, then repeat without releasing. | Transient lock: publish the same captured bytes successfully. Persistent lock: explicit failure after at most six attempts/620 ms scheduled delay, no recapture or false success. Collision/nonretryable error: preserve existing content and fail. Acceptance integrity errors identify the Artifact EntityRef. |

For the next **real Copilot** run, retain the `work MCP protocol negotiated`
record with its offered/selected versions and invocation ID. Require actual
bound tools to initialize and J1 to finish before proceeding through J2-J8.
A controlled actor accepting negotiation proves the transport behavior, not
that the real model completed the product journey. The original publication
lock holder remains unknown; preserve OS error and stage information if it
recurs rather than masking it with another successful whole-test retry.

### Round 3 follow-up gates

Round 3 proved real MCP negotiation and the J1 diagnosis/delegation boundary.
It did not admit an initial real-model plan or run a worker. Preserve the
original long-path failure and the separately counted shorter-path control;
neither is an uninterrupted J2-J8 journey.

| Gate | Trigger / boundary | Required oracle and negative control |
|---|---|---|
| Discoverable plan output kinds | Obtain `plan_propose` through real HTTP `tools/list`; submit an invalid output kind, then correct it using the advertised vocabulary. | All six supported kinds are advertised. Invalid proposals return `INVALID_ARGUMENT`, exact indexed field feedback and allowed values without changing the work or admitting tasks. A corrected proposal applies and starts a worker. Code/Tree/GitCommit without a required check still fail. |
| Approved prose evidence mapping | Approve an ordinary-language evidence rule; copy it exactly into task criterion `evidenceRule`, with concrete `requiredEvidence` bindings. | The immutable task/dispatch retains the exact rule, and actual checks/captured artifacts govern acceptance. Changed/omitted prose, empty/unknown/optional-only bindings fail. Existing command, artifact and bare gate reference rules cannot be replaced by weaker mappings. Run real ACP conformance with both prose and reference-based works through failed-check/rework/delivery. |
| Long managed Git worktrees | Provision under managed workspace paths of at least 237 and 250 characters, then capture a fixed commit and run Git in the populated worktree. | Production provisioning and fixed capture succeed without source checkout/global Git configuration changes. Checkout failure remains explicit and removes partial worktree registration where possible; source-repository branch references are retained, and cleanup failure is reported. Keep artifacts inside the managed root. Do not substitute a shortened-state run or only set core.longpaths and call the original error fixed. |
| HTTP response shutdown | Use real Windows TCP with queued input after a complete notification or JSON request. | Complete 202/JSON response and clean EOF, not 10054; maintain authorization, framing and timeout limits. Retain per-exchange/server diagnostics. The reproduced queued-input mechanism does not establish the exact historical trigger; retain any new reset rather than hiding it with a passing rerun. |

The next real-model run must show an admitted/applied plan, a dispatched worker,
and typed coordination completion before claiming the J2 execution boundary.
Record exact rejected field paths and version conflicts if planning still
fails. Do not inject a valid plan, increase the deadline, or relax approved
scope/evidence constraints to turn it green. The new prose mapping preserves
meaning and concrete references; semantic adequacy of the chosen check still
requires the real journey and delivery review.

### Round 4 follow-up gates

Round 4 admitted A/B plans and dispatched four real workers, but the ACP host
cancelled every initial acknowledgement permission request. Its provider-side
"user rejected" text was a host-policy decision, not an actual human rejection.
The old scripted actor bypassed that boundary by calling MCP directly.

| Gate | Trigger / boundary | Required oracle and negative control |
|---|---|---|
| Initial acknowledgement permission | Real ACP `session/request_permission` for the bound acknowledgement, then HTTP MCP `task_acknowledge`. Include Copilot's observed bare title with kind `other` and the server-qualified forms. | Select `allow_once`, then observe the service Acknowledgment and acknowledged Attempt. Permission alone must not set acknowledged. Deny wrong session/server/tool/dispatch/revision/continuation, malformed arguments, unbound tools and native execution/write requests before acknowledgement. Never substitute global permission approval. |
| Continuation and revocation | Yield for internal context, resume with the current continuation, and request permission before acknowledging it. Repeat after cancellation/release/end of turn. | Exact current continuation succeeds; missing/stale continuation and revoked/nonrunning invocations fail. Actual resumed text starts at chunk zero for its new part; no ignored TextDelta errors. Existing post-acknowledgement execution remains available. |
| Actual producing worker | Use the latest Dev binary with a bounded real Copilot worker on a synthetic project. | Record actual permission request/response, acknowledgement, producing result and real check/candidate. A scripted coordinator may prepare a diagnostic plan, but that is explicitly a worker-path probe, not an autonomous planning or J1-J8 pass. Retain failed probes and count every model prompt. |

The repair probe used two authorized real worker prompts: the first exposed
Copilot's bare permission title and was denied by the initial qualified-only
matcher; the second acknowledged, produced a result, passed its actual command
check and created a candidate. The coordinator was scripted, and no final human
acceptance, real-model rework or complete native journey was exercised.
Repeat the uninterrupted real two-work journey separately. Native foreground
acquisition and the previously recorded TAEF initialization failure remain
independent verification gaps; do not bypass safe input preconditions.

### Round 5 follow-up gates

Round 5 confirmed the real acknowledgement repair and one passing native code
check. Both works then submitted File-only report/evidence results whose command
checks failed before launch: evaluation omitted the producer's fixed dependencies,
and runtime required a Tree/GitCommit. Generic evidence collection repeated the
same error. Refusing a mutable workspace fallback was correct.

| Gate | Trigger / boundary | Required oracle and negative control |
|---|---|---|
| File-only checks and report delivery | Admit Report/Evidence outputs, capture two Files, change the synthetic mutable workspace, and run the declared command through the runtime. | Real process reads the captured files and reaches a Report candidate and explicit acceptance. Uncaptured workspace files are absent; modified workspace bytes do not affect the verdict. File paths are captured basenames; nested layouts use Tree captures. |
| Pinned integration inputs | An accepted code contribution feeds a ReadOnly integration that submits only File reports. Include two input aliases of the same immutable artifact. | Evaluation retains both exact original input bindings and current outputs in its manifest; the real command reads the accepted code plus fixed reports. LocalCode candidate retains the unambiguous accepted source snapshot and provenance; acceptance rechecks it. Changed source bindings or captured code block acceptance. |
| Ambiguous and complete snapshots | Submit conflicting same-layer paths, duplicate identical captures, and a replacement code snapshot that deletes an old input file while retaining a separately pinned File check script. | Conflicting captures fail explicitly before process launch; identical aliases remain usable. A complete new code snapshot does not resurrect deleted dependency files and still runs its independent check script. No arbitrary artifact order or mutable cwd decides the result. |
| Preparation diagnostics and bounded correction | Trigger a real preparation/startup error without a process GateResult; read result/rework and coordinator-visible diagnostics. | Preserve the exact bounded error, phase and affected identities. Explicit ReviseOutput dispatches corrected capture work without weakening its contract; repaired transient causes can explicitly retry CollectEvidence. Identical repeated failures reach a bounded blocker. Actual nonzero process results and human cancellation retain their distinct classifications. |

The deterministic ACP/HTTP/native-process fixture covers the new File-only and
pinned-code report paths through final acceptance alongside the existing
context/rework/decline paths. This is not a new live-model or packaged UI
sign-off. Obtain fresh package/model-budget authorization before repeating the
autonomous two-work journey; the two prompts authorized for the round-4
diagnostic probe are exhausted and cannot be reused as authorization.

## 3. One real two-work product journey

Use one real code project and two independent works. Choose defects that are
actually present; do not ask a scripted actor to emit pretend discoveries,
pretend check failures, or preset completion cards.

| Step | User action | Required downstream evidence |
|---|---|---|
| J1: question -> delegation | Ask about a real error, then request its repair without restating the supplied material. | The answer does not create an unwanted work. The subsequent brief carries the correct material, boundaries and criteria; explicit approval precedes execution. |
| J2: independent work | Create B and continue drafting there while A executes. | Distinct workspaces, histories, tasks and artifacts. A advances without target changes or a hidden human scheduler. |
| J3: real finding -> changed arrangement | Let A investigate and discover a fact affecting its initial approach. | The worker records the finding; the coordinator obtains its body/evidence and records an appropriate plan/action. No copied log or manually supplied next step. |
| J4: internal clarification | An executor needs a fact already present in approved material. | Correct internal question/answer and acknowledged continuation; no unnecessary human decision. Include the early-answer-before-yield timing case. |
| J5: failed check -> fresh submission | A real required check fails. | Captured diagnostic bytes reach coordination/execution, correction is bound to the finding, and a fresh result is checked against its own content. No weakening of criteria. |
| J6: actual human judgment | A needs a genuine scope/input choice while the user is drafting in B. | Persistent A attention survives routine events and B inbox refresh. Answer is applied to A; returning to B restores its draft. Same-grant scope revision is proposed and explicitly approved, not silently applied. |
| J7: inspect -> contribute -> hand back | Before final delivery, inspect A, take over, make a small real edit, and hand it back. | Accurate workspace, settled old writer, explicit human ownership, real immutable manual capture, and renewed relevant work/checks. Record the current conservative invalidation cost honestly. |
| J8: final revision -> actual delivery | Request an explanation/report correction on a candidate, review its replacement, then accept A and inspect B's report. | New fixed candidate/version with coherent evidence and explicit retained/invalidated outputs. Code has a real location and version; report contents are readable. Acceptance does not mean external publication. |

If there is no naturally occurring check failure or internal clarification,
record that the scenario did not exercise that gate. Use another known-bad
fixture/project variant; do not insert false evidence and call it autonomous.

## 4. Human-assistance ledger

Record every intervention, not just the final outcome:

| Time / work | Category | Exact action | Why needed | Would work advance without it? | Evidence IDs |
|---|---|---|---|---|---|
| Fill during run | Goal clarification / business judgment / voluntary inspection / unnecessary prompting / information copying / manual scheduling / repeated goal | User input or manual action | Concrete reason | Yes / no / unknown | Request, event, report, artifact, candidate |

The last four categories are responsibility-transfer failures when needed for
ordinary internal progress. Necessary goal clarification and genuine human
judgment are allowed and counted separately. Normal internal handoffs target
zero unnecessary prompting, zero information copying, and zero repeated goal
explanation.

Report component conformance, live-model behavior, and UI observations
separately. A scripted coordinator cannot earn the autonomous-journey result.

## 5. Packaged integration and release-report wiring

Use `test\e2e\ItE2E` for process/package/UI assertions. Before running live:

```powershell
$env:ITE2E_PACKAGE = 'Dev' # only after explicit selection
pwsh -File test\e2e\bootstrap.ps1 -Check
```

When adding packaged regression cases, use the exact R1-R9 checklist titles
above where the suite actually crosses the required boundary. Add unchecked
`[E2E]` rows to `doc\release-check-list.md`, assign IDs with
`test\e2e\Set-ChecklistIds.ps1`, and update the suite table. Do not manually
invent/renumber checklist IDs or mark a case covered merely because a Rust
test with a similar name passed.

Run the added suite through `Invoke-ItE2EReport.ps1`, verify the generated
checkboxes, and verify incremental `Update-ReleaseReport.ps1` mapping.
Keep real-model quota-consuming trials outside default published/CI discovery.

## 6. Sign-off and output

The next verification report must include:

- Exact build/package/provider provenance and settings/source preservation.
- One result per R1-R9 and J1-J8: PASS, FAIL, BLOCKED, or NOT RUN, with evidence.
- Actual regression names, commands and logs; no reused green counts from an
  older executable.
- Before/after acceptance state for B1/B2; received report/diagnostic contents
  for B3; visible attention and draft preservation for B4; real ownership and
  capture sequence for B5; terminal state plus reason for B6.
- The full human-assistance ledger and remaining product limitations.
- Cleanup outcome for test-owned resources only.

Do not sign off if any confirmed defect still reproduces, if the deployed
build cannot be identified, or if required internal progress depended on human
orchestration. Native test-host failures before test execution are BLOCKED,
not product passes or failures. Missing provider approval/quota makes the
real-model stage BLOCKED; a connected product behaving incorrectly is FAIL,
not SKIP.
