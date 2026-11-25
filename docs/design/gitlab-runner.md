# GitLab CI jobs on the slot pool — design

Status: proposal, 2026-11-25, against slot 0.36.0. Nothing here is built. No runner was registered,
no `gitlab-runner` was installed and no token was created for this document.

Sources: GitLab documentation read on 2026-11-25 at docs.gitlab.com (it already names GitLab 19.7,
so a fact without a version may be newer than the instance's), and the fleeting source on
gitlab.com where the docs say nothing. Each fact carries its page. *(unverified)* marks a claim
taken from memory of a tool's behaviour or reasoned from the docs without a run; § 9 names the check
for each one that matters. The instance in question runs GitLab 19.6.1 (Enterprise Edition); its
licence tier is not known here, so every option names the tier it needs.

Terms: the **workstation** runs the local daemon and the agent sessions; the **node** is the cloud
build machine of `contrib/node`; the **pool** is the slots of both. A **CI job** is a job of a
GitLab pipeline; a **slot job** is a job in the pool. The **runner manager** is the `gitlab-runner`
process that asks GitLab for CI jobs.

## 1. Question and short answer

Can the pool take CI jobs of a GitLab instance when it has free slots, so pipelines build on
capacity that is already paid for?

Yes, with a stock `gitlab-runner` on the workstation whose custom executor is a new `slot ci`
command (option A, § 3). The runner manager talks to GitLab; slot only places and runs the
commands of a job. The steps that use the job's token (clone, cache, artifacts) stay on the
workstation; only the job's own script crosses to the node, synced like any remote build, in the
sandbox, without a credential. Every CI job is a slot job at low priority, so an agent session's
build goes first.

What the measurement says (§ 8): CI jobs on the instance wait 3 s for a runner at the median and 6 s
at p90. The pool would not shorten the queue; what it could change is run time and where the job
hours are paid. Run time on the node is not measured yet; phase 1 (§ 9) measures it on one project.

## 2. What GitLab offers

- **Executors.** Docker, Docker Autoscaler, Instance and Kubernetes are developed; SSH, Shell,
  Parallels, VirtualBox and Custom are in maintenance mode ("critical security updates but no new
  features"); GitLab accepts no new executors. https://docs.gitlab.com/runner/executors/
- **Custom executor.** `config_exec`, `prepare_exec`, `run_exec` (required), `cleanup_exec`;
  `run_exec` is called once per sub-stage with the script path and the stage name:
  `prepare_script`, `get_sources`, `restore_cache`, `download_artifacts`, `step_*`,
  `build_script`, `after_script`, `archive_cache`, `upload_artifacts_on_*`,
  `cleanup_file_variables`. Exit with `$BUILD_FAILURE_EXIT_CODE` or `$SYSTEM_FAILURE_EXIT_CODE`;
  CI variables arrive as `CUSTOM_ENV_*`; `JOB_RESPONSE_FILE` holds the whole job, job token
  included. https://docs.gitlab.com/runner/executors/custom/
- **`prepare_timeout`** (Runner 19.0.0) bounds the prepare stage for executors that "wait for an
  external resource scheduler", so a stalled job does not hold a runner slot for the whole job
  timeout. https://docs.gitlab.com/runner/configuration/advanced-configuration/#prepare-stage-timeout
- **Instance and Docker Autoscaler executors** (experiment in Runner 15.11, beta 16.6, GA 17.1) take
  machines from a fleeting plugin: one Go interface (`Increase`, `Decrease`, `ConnectInfo`,
  `Heartbeat`, …) behind a gRPC plugin process; the runner reaches each machine over ssh.
  `preemptive_mode` requests a job only when a machine is confirmed free.
  https://docs.gitlab.com/runner/executors/instance/,
  https://gitlab.com/gitlab-org/fleeting/fleeting/-/blob/main/provider/interface.go. A static
  plugin over a fixed host list exists (`gitlab-org/fleeting/fleeting-plugin-static`); it is not on
  the docs' plugin list and says it does no cleanup between jobs.
- **Concurrency.** `concurrent` (global), `limit` (per runner entry), `request_concurrency`
  (default 1), `check_interval` (default 3 s). At `concurrent` or `limit` the runner stops asking;
  the CI job stays `pending` in GitLab and is dropped after 24 h when a matching runner exists.
  https://docs.gitlab.com/runner/configuration/advanced-configuration/#how-check_interval-works,
  https://docs.gitlab.com/ci/jobs/job_troubleshooting/
- **Job request API.** `POST /api/v4/jobs/request` appears only on the long-polling page and in the
  development docs; the REST reference does not document it, and no page calls it stable for
  other clients. https://docs.gitlab.com/ci/runners/long_polling/,
  https://docs.gitlab.com/development/cicd/#job-scheduling
- **Job router and runner controllers.** The job router (part of GitLab Relay, formerly KAS) hands
  jobs to runners instead of polling: Ultimate, beta since GitLab 19.6, Runner 18.9 or later with
  `FF_USE_JOB_ROUTER`. A runner controller dials the router and admits or rejects each job; a
  rejected job fails with `job_router_failure`: Ultimate, self-managed and Dedicated only,
  experiment since 18.9, instance runners only, configured through the administrator REST API.
  https://docs.gitlab.com/ci/runners/job_router/,
  https://docs.gitlab.com/ci/runners/job_router/runner_controllers/
- **ID tokens** (`id_tokens:`, every tier) carry `project_path`, `ref`, `ref_protected`,
  `runner_id` and more, signed RS256, valid until the job timeout.
  https://docs.gitlab.com/ci/secrets/id_token_authentication/

## 3. Options: how a job reaches the pool

| | how | tier | the job token | slot's part |
|---|---|---|---|---|
| A | stock runner on the workstation, custom executor = `slot ci` | Free | workstation only | a driver: places stages, filters what crosses |
| B | stock runner on the node, Docker or Shell executor; jobs reach the jobserver through `slot gate` | Free | on the node | the gate (built, design.md § 19) |
| C | runner on the workstation, Instance or Docker Autoscaler executor, a fleeting plugin that offers the node | Free | on the node | a fleeting plugin over one host |
| D | slot asks `POST /api/v4/jobs/request` itself and runs the job | Free | wherever slot runs it | a second runner |
| E | A, plus a runner controller that admits a job only when the pool has room | Ultimate, self-managed, experiment | as A | an admission service |

- **B** puts the runner authentication token and every job token on the node, against the node's
  rule that no credential goes there (`contrib/node/README.md`). Shell gives "limited isolation";
  Docker needs a daemon on the node and root-equivalent setup.
- **C** also runs the whole job on the node, clone and artifact upload included, so the job token
  is there; the plugin unit is a machine, not a slot: the runner would count capacity in machines
  while the pool counts slots, two schedulers for one node. The static plugin does no cleanup.
- **D** re-implements the runner against an undocumented internal API: trace upload, artifacts,
  caches, cancel, retries. slot would become a second runner. Rejected.
- **E** is A with one gain: GitLab keeps a job pending, or gives it to another runner, when the
  pool says no, where A has already taken it. It needs Ultimate, an instance runner (the widest
  scope, § 5) and an administrator; the feature is an experiment. Kept as a later step if the tier
  allows.

**Recommended: A.** It is the only option that keeps every credential off the node and needs no
paid tier. The risk is the custom executor's maintenance mode: no new features, security fixes
only. The interface slot uses (four programs, exit codes, `CUSTOM_ENV_*`) is the whole contract, so
a removal would be announced as a breaking change *(unverified: GitLab's deprecation practice, not
read for this executor)*.

## 4. Option A in detail

### 4.1 Where each stage runs

The runner manager runs on the workstation as a user service with `builds_dir` under slot's state
directory. `config.toml` names `slot ci config`, `slot ci prepare`, `slot ci run`, `slot ci
cleanup`.

| stage | where | why |
|---|---|---|
| `config` | workstation | answers `builds_dir`, `cache_dir`, `builds_dir_is_shared = false`; refuses a denied project (§ 5.2) |
| `prepare` | workstation | checks the node is placed-on (up, not draining, deadline far enough, § 6); queues nothing |
| `prepare_script`, `get_sources`, `restore_cache`, `download_artifacts` | workstation, outside the pool | the runner's own script; uses the job token; network work, not compile work |
| `step_*`, `build_script`, `after_script` | node, a slot job | the project's code; § 4.2 |
| `archive_cache*`, `upload_artifacts_*`, `cleanup_file_variables` | workstation, outside the pool | the job token again |
| `cleanup` | workstation and node | removes the checkout and the node's mirror of it |

### 4.2 The script that crosses

The runner's generated stage script exports every variable inline, the job token and
`CI_REPOSITORY_URL` (which carries the token) among them *(unverified: read from the custom
executor page's statement that the script sets up the job's environment, not from a generated
script)*. So `slot ci run` does not send that script. It reads `JOB_RESPONSE_FILE` and composes the
stage itself: the step's script lines, preceded by exports of the variables that may cross.

A variable crosses when all hold:

1. it is not `masked`, not a `file` variable, not from `id_tokens`;
2. its name is not one of `CI_JOB_TOKEN`, `CI_REPOSITORY_URL`, `CI_REGISTRY_PASSWORD`,
   `CI_DEPENDENCY_PROXY_PASSWORD`, `CI_JOB_JWT*`, nor ends in `_TOKEN`, `_PASSWORD`, `_SECRET`,
   `_KEY`;
3. it is a predefined variable or one the project lists in `SLOT_CI_ENV` (the same opt-in as
   `SLOT_ENV` for remote builds, AGENTS.md § Boundary).

A script that needs anything else fails on the node with one line naming the variable; that CI
job's tags then keep it off this runner. The checkout crosses with the existing sync (design.md
§ 16, `docs/design/remote-pool.md` § 4); files the script writes under `artifacts:paths` or
`cache:paths` come back with the existing `--fetch` before the upload stage runs.

Exit codes: the script's code goes to `$BUILD_EXIT_CODE_FILE` and the stage exits
`$BUILD_FAILURE_EXIT_CODE`; a failed sync, a node that is gone, or a slot job ended by the
daemon exits `$SYSTEM_FAILURE_EXIT_CODE`, which a pipeline can retry with
`retry: when: runner_system_failure`.

### 4.3 Isolation on the node

A CI job runs code of whoever may push to the project. The runner security page: such a user
"could compromise the security of the environment hosting the runner", worst on non-ephemeral
runners used by several projects. https://docs.gitlab.com/runner/security/

Every CI slot job runs in the sandbox of design.md § 27, with a CI profile on top:

- **Its own workspace root** for CI mirrors (`<store>/ci`), apart from the agents' mirrors; each
  job sees only its own mirror, as today.
- **The home hidden**: a tmpfs over the build user's home, with only `~/.rustup` and
  `~/.cargo/bin` bound back read-only. The node's home copies (Maven, Go modules, cargo git
  sources of private repositories, tool caches) are not visible: they hold other projects' code.
- **Its own `CARGO_HOME`, `GOPATH`, `GOCACHE`** under the job's mirror, gone at cleanup.
- **The cloud metadata address blocked** for the build user (169.254.169.254, an nftables rule
  by owner, set by `provision.sh`). No agent build needs it; a CI job must not read the
  instance's role through it.
- **The compile cache**: a CI job either uses a second sccache server with its own store, or none.
  Sharing the agents' store lets a CI job's compiles land in the cache every agent build reads.
  This touches the rule that every Rust build on a host uses one cache; decision D4.

Same uid, separate mount namespaces: the sandbox hides paths, it does not change the user. A
kernel escape from bwrap reaches the build user's files on the node, which hold no credential by
the node's rule.

## 5. Scope and denied origins

### 5.1 Which runner

- **Project runner, locked**, enabled on a listed set of projects; instance and group runners
  cannot be locked and reach every project of their scope.
  https://docs.gitlab.com/ci/runners/runners_scope/
- **Tag `slot`, `run_untagged = false`**: a CI job lands here only when its project asks for it.
  https://docs.gitlab.com/ci/runners/configure_runners/#how-the-runner-uses-tags
- **Protected or not** (`access_level`): `ref_protected` runs only jobs of protected branches and
  tags; merge request pipelines of any developer's branch then stay on the shared runners.
  https://docs.gitlab.com/api/runners/#update-runners-details. Decision D3.
- **`maximum_timeout` = 30 min**, the longest run time a slot job may name (`slot run-limit`, `--max 30m`).

### 5.2 A denied origin never reaches the node

Three checks, each enforced in code, not assumed from configuration:

1. `slot ci config` compares `CUSTOM_ENV_CI_SERVER_HOST` and `CUSTOM_ENV_CI_PROJECT_PATH` with
   the remote deny lists (`remote-deny-origins`, `remote-deny`) before the clone, and fails the job
   with one line. It runs on the workstation, so nothing of the project has been fetched.
2. The sync to the node refuses a denied tree, as for every remote build (the deny check of
   remote mode).
3. No cache crosses projects: GitLab keeps the project id in every cache path
   (https://docs.gitlab.com/ci/caching/), the CI profile hides the node's home copies, and the
   CI compile cache is separate or off (D4).

The runner being enabled only on allowed projects is the fourth line, and the only one that is
configuration.

## 6. Admission and lifetime

- **One CI job = one slot job** for its node stages, at priority `low`, holding `SLOT_CI_TOKENS`
  slots (a CI variable, default 4, capped at the slots in circulation). The pool's headroom,
  admission share and run-time limit apply unchanged.
- **Agents first**: `low` CI jobs are exempt from `promote-after`, so a waiting CI job never
  becomes `high` and overtakes a session's build.
- **`concurrent` = `limit` = 2 in phase 1**, `request_concurrency = 1`. The runner then holds at
  most two CI jobs; more stay `pending` in GitLab. A held job that waits in the pool shows `slot:
  job N queued` in its GitLab log.
- **A bounded wait in the pool**: `prepare_timeout` (Runner 19.0) and the job's own wait bound in
  `slot ci run` (15 min) end a CI job that cannot get slots with a system failure, so it can be
  retried on another runner.
- **Asking only when there is room** *(unverified)*: the runner re-reads `config.toml` when it
  changes; slot could write `limit` from its free slots each minute so the runner stops asking while
  the pool is busy. Phase 0 checks the reload; without it, `concurrent` stays small and fixed.
- **Node lifetime**: `cluster.toml` holds the node's deadline. slot sends the runner manager
  SIGQUIT (stop asking, finish running jobs; https://docs.gitlab.com/runner/commands/#signals) 40
  minutes before it: 30 minutes of the longest run time plus margin. A node lost mid-job ends the
  slot job; the stage exits as a system failure, and GitLab retries it when the pipeline sets
  `retry: when: [runner_system_failure, runner_interrupted]` (`runner_interrupted` from GitLab
  19.1, https://docs.gitlab.com/ci/yaml/#retrywhen).
- **The workstation alone takes no CI job**: its home holds the session's credentials and every
  agent tree, and the sandbox binds `/` read-only, not hidden. While no node is placed-on, `slot ci
  prepare` fails the job as a system failure and slot stops the runner manager. Decision D5.

## 7. Credentials

| credential | whose | where | lifetime | a leak lets someone |
|---|---|---|---|---|
| runner authentication token (`glrt-`) | the account that creates the project runner | workstation, `config.toml`, mode 0600 | until rotated or revoked; expiry and rotation deadline need Premium | ask for this runner's CI jobs: receive their source, job tokens and every CI variable those jobs get, protected ones on protected refs |
| CI job token (`glcbt-`) | the user who triggered the pipeline, limited endpoints | workstation, inside the runner's stages | while the job runs (JWT format: timeout + 5 min) | read and write what that token's endpoints allow in the job's project and allowlisted ones |
| none | | node | | |

https://docs.gitlab.com/security/tokens/, https://docs.gitlab.com/ci/jobs/ci_job_token/,
https://docs.gitlab.com/api/users/#create-a-runner-linked-to-a-user

Creating the runner, holding its token on the workstation, and whose account it is linked to is
decision D2.

## 8. What was measured

Read-only from the jobs API of the instance, 2026-11-25 01:42 CET: the last 300 finished jobs of
each of 8 projects of one group, 2,397 jobs from 2026-10-29 to 2026-11-25, all on the instance's
shared Kubernetes runners.

| | p50 | p90 | max | sum |
|---|---|---|---|---|
| queue wait (`queued_duration`) | 3 s | 6 s | 276 s | 3.0 h |
| run time (`duration`) | 33 s | 169 s | 3,600 s | 55.0 h |

- The largest job by summed run time: a `verify` job, 50 runs, mean 576 s, 8.0 h.
- A Rust gate on the high-performance runner entry: 37 runs, mean 450 s, max 618 s, 4.6 h.
- 1,141 of the 2,397 jobs ran on 2026-11-24, a day of many merges.

The pool, from `slot usage --since 24h` at the same time: 3,367 slot jobs, 84.5 slot-hours used.
The workstation alone circulates 32 slots, 768 slot-hours a day; the node adds 32 slots while it
runs.

So: the queue on the shared runners is short and the pool would not shorten it. The pool has room
for the 55 job-hours of this 27-day sample many times over. What is not known is how long the same CI job runs
on the node; phase 1 measures it.

## 9. Phases

**Phase 0, checks with no runner registered:**

1. Read one generated stage script of the custom executor from the runner's source and confirm
   which variables it exports (§ 4.2).
2. Confirm whether `gitlab-runner` reloads `config.toml` on change, from its source (§ 6).
3. Confirm the variable fields in the job response (`masked`, `file`, `public`) from the runner's
   source (§ 4.2).

**Phase 1, one project:** one locked project runner, tag `slot`, `concurrent = 2`, one Rust
gate job tagged into it. Measure run time against the same job on the shared runners over a
week, read from the jobs API.

**Phase 2:** more projects, `concurrent` from the measured share; option E if the tier is
Ultimate and the job router leaves experiment.

## 10. Not covered

- Docker-in-CI (`image:`, `services:`): the node runs no container daemon; a CI job that names an
  image fails in `config` with one line. Projects whose jobs need an image stay on the shared
  runners.
- Windows and macOS jobs.
- Jobs that deploy, publish or need secrets: § 4.2 keeps their secrets off the node, so they fail
  there by name.

## 11. Decisions for the operator

- **D1 scope**: which projects get the runner. Default proposal: one project of § 9 phase 1.
- **D2 the runner token**: created by which account, held on the workstation in `config.toml`.
- **D3 protected refs**: `ref_protected` (only protected branches and tags) or `not_protected`
  (merge request pipelines too, from every developer's branch).
- **D4 the compile cache of CI jobs**: a second sccache store on the node, or none.
- **D5 the workstation**: no CI job runs there (proposed), or it serves CI jobs while no node runs.
