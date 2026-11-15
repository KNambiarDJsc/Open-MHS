# Ecosystem landscape (researched 2026-11-15)

What exists, so that Open-MHS adds something rather than rebuilding it. "Verified" below means I read the project's
own documentation or code page; "vendor claim" means a press release or marketing page; "unverified" means a
third-party summary. Several items are less than three months old and may change.

## Where each system sits

| Layer | Authoritative today | Open-MHS role |
|---|---|---|
| Real-time control | ros2_control, robot controllers, safety PLCs | **None.** Never in this loop |
| Task execution | ROS 2 actions, Nav2, MoveIt 2, behaviour trees, Open-RMF, vendor fleet managers, OpenRAL (agentic harness) | Observe and correlate their task/action ids |
| Recording | rosbag2 with MCAP | Reference recordings by URI and time range; never copy them |
| Visualization and log review | Foxglove, Rerun, PlotJuggler | Deep-link into them |
| Robotics data platforms with triage | Foxglove Data Platform, Roboto, Sift | **Direct overlap.** Integrate, do not out-build |
| Fleet operations | Formant, InOrbit (and OpenRobOps), Viam, Open-RMF dashboards | **Overlap on incidents and fleet views** |
| ROS 2 diagnostics | `diagnostics`, `ros2_medkit` (fault manager, action-status bridge) | Consume as a source |
| Service observability | OpenTelemetry, Prometheus/Grafana, Sentry | Interoperate (trace ids, OTLP) |
| Robot learning | LeRobot (dataset v3), robomimic, Open X-Embodiment, GR00T, Isaac Lab / Isaac Lab-Arena, MuJoCo, ManiSkill | Hand off references and metadata; never train |

## Findings that change the design

1. **Agentic triage of robot logs is already sold.** Roboto describes agents that review every new bag, tag failure
   modes and open evidence-backed tickets in Linear, GitLab or Jira, supporting ROS, PX4, MCAP and Parquet (vendor
   claim; Roboto's ROS Discourse post). Foxglove relaunched in August 2026 as an "agentic data platform for physical
   AI" with search, curation, event tagging and bring-your-own-storage (vendor claim). **Consequence:** "an AI reads
   your bags and explains the failure" is not a differentiator for Open-MHS, and building it would be a losing race.
2. **Open-source robot operations with incident management now exists.** InOrbit released OpenRobOps under Apache-2.0
   on 2026-09-15, with telemetry, diagnostics, remote operation, incident management and an ISO 21423 reference
   implementation (press coverage; ISO 21423:2026 reported published 2026-11-03). **Consequence:** "open-source fleet
   ops" is not empty ground either. Open-MHS should be a source and analysis layer that can feed it, not a rival
   dashboard.
3. **ROS 2 fault storage is being standardised by others.** `ros2_medkit` (on index.ros.org for Jazzy) has a fault
   manager with SQLite persistence, debouncing, a `/diagnostics` bridge, a log bridge and an action-status bridge, and a
   Foxglove integration (verified package pages). **Consequence:** consume its faults; do not write a ROS fault manager.
4. **Action status is available but off by default for introspection.** Every action server publishes
   `/<action>/_action/status` (`action_msgs/GoalStatusArray`), meant for introspection; goal/result payload
   introspection is opt-in from Kilted (verified ROS 2 docs). Open-MHS's own experiment found rosbag2 (Jazzy) records
   the status topic with an empty schema (`research/outcome-verifier/ros2-evaluation/RESULTS_GAZEBO.md`).
   **Consequence:** goal UUIDs are the strongest join key ROS 2 offers for free; the importer must tolerate missing
   schemas.
5. **Nobody in the results above publishes release-to-release outcome comparison with stated denominators and
   uncertainty.** Formant's analytics report fleet uptime, incident rates and missions per robot (vendor claims and a
   third-party summary); Viam supports fleet-wide SQL over synced data (verified docs). None of the sources found
   compares task success between software/model releases with exposure-based denominators and intervals. This is an
   absence in what I searched, not proof of absence.
6. **Intervention data exists in learning stacks, not in operations stacks.** LeRobot's HIL-SERL example stores an
   `is_intervention` flag per transition in `complementary_info` (example code, not documented schema); LeRobot dataset
   v3 keeps episode boundaries as offsets in Parquet metadata (verified docs). Fleet tools record interventions as
   operator actions, not as training-ready episode segments. **Consequence:** an intervention record that carries both
   operational context (robot, release, incident) and dataset references (episode, frame range) has a gap to fill.
7. **OpenTelemetry for ROS 2 is community-level.** A `ros-opentelemetry` library was announced on ROS Discourse;
   `ros2_tracing` is LTTng-based and I found no native OTLP export (unverified beyond the announcement).
   **Consequence:** carry W3C trace ids when present; do not depend on them.
8. **Evaluation in simulation is consolidating.** NVIDIA Isaac Lab-Arena evaluates GR00T N models and hosts benchmark
   suites (RoboCasa, LIBERO, RoboTwin), with sim-to-real validation "in partnership", not yet published (verified NVIDIA
   blog and docs). **Consequence:** record evaluation runs by reference (suite, task, seed, checkpoint); do not run them.
9. **Agentic harnesses are appearing above ROS 2.** OpenRAL ("Robot Agentic Layer", ROS 2-native, skills packaged as
   Hugging Face repos; licence statements conflict) and Strands Robots (AWS Strands Labs, experimental, LeRobot and
   GR00T integration) both plan and call skills. **Consequence:** they are event sources with task and skill ids;
   design the envelope so a skill call is an action.
10. **Intermittent connectivity is an open question even in Open-RMF.** A 2026 Open-RMF Discourse thread asks how a
    fleet adapter should behave when a robot disconnects after accepting a task, with no answer in the thread.
    Local-first buffering with later resynchronisation is a real need, but whether teams want *another* agent for it
    is unproven.

## What this leaves for Open-MHS

Not visualization, not log storage, not AI triage, not fleet dashboards, not a ROS fault manager. What remains thin is:

- a **vendor-neutral, versioned event schema** that ties task attempts, ROS goal ids, interventions and faults to an
  immutable **deployment manifest** (software, image, model/policy revision, firmware, hardware, config, calibration);
- **release intelligence with honest statistics**: exposure-based denominators, intervals, stratification, and
  "insufficient evidence" as a first-class answer;
- **explicit uncertainty and provenance** on every outcome (reported vs verified vs unknown; observed vs inferred vs
  operator vs AI hypothesis), which Open-MHS already practises in its lifecycle and outcome verifier;
- **incident evidence packages** that link to recordings in Foxglove/Roboto and to dataset episodes in LeRobot.

Whether teams will pay for, or even adopt, that thin layer is the open question (see
[09-critical-review.md](09-critical-review.md)).

## Sources

- Foxglove: [agentic data platform](https://foxglove.dev/blog/introducing-the-agentic-data-platform-for-physical-ai),
  [product](https://foxglove.dev/product), [data search and curation launch](https://roboticsandautomationnews.com/2026/04/22/foxglove-launches-unified-data-platform-to-accelerate-physical-ai-development/100847/),
  [ANYbotics case](https://foxglove.dev/customers/anybotics), [ros2_medkit in Foxglove](https://foxglove.dev/blog/sovd-diagnostics-and-ota-updates-in-foxglove-via-ros2-medkit)
- Roboto: [Roboto Agents on ROS Discourse](https://discourse.openrobotics.org/t/roboto-agents-agentic-triage-root-cause-analysis-and-data-curation-for-rosbags-and-mcap/57438),
  [platform](https://roboto-astro.pages.dev/platform)
- Sift: [what is Sift](https://docs.siftstack.com/overview/what-is-sift), [Sift Edge](https://www.siftstack.com/mission-critical/sift-edge-early-access),
  [2026 winter release](https://www.siftstack.com/mission-critical/sift-2026-winter-release)
- InOrbit: [OpenRobOps release](https://www.therobotreport.com/inorbit-ai-releases-openrobops-iso-21423-reference-implementation/),
  [OpenRobOps open source](https://www.robotics247.com/article/inorbit.ai-releases-openrobops-open-source-robot-operations-software),
  [advanced incidents](https://www.inorbit.ai/blog/manage-complex-states-with-advanced-incidents),
  [ISO 21423 explainer](https://www.automate.org/robotics/industry-insights/the-robot-industry-is-finally-getting-a-common-language)
- Formant: [analytics launch](https://www.therobotreport.com/formant-launches-analytics-feature-optimize-fleet-performance/),
  [workflows](https://formant.io/?p=20008), [third-party summary](https://www.rfp.wiki/artificial-intelligence/robotics-ai-development-platforms/formant)
- Viam: [data management](https://docs.viam.com/data/overview/), [operational intelligence](https://www.viam.com/post/from-data-collection-to-operational-intelligence-how-deployed-robots-become-your-best-training-ground)
- ROS 2: [action introspection (Kilted)](https://docs.ros.org/en/kilted/Tutorials/Demos/Action-Introspection.html),
  [actions design](https://design.ros2.org/articles/actions.html), [ros2_medkit](https://github.com/selfpatch/ros2_medkit),
  [medkit fault manager (Jazzy)](https://docs.ros.org/en/jazzy/p/ros2_medkit_fault_manager/),
  [medkit action status bridge](https://index.ros.org/p/ros2_medkit_action_status_bridge/),
  [ROS 2 x OpenTelemetry](https://discourse.openrobotics.org/t/ros2-x-opentelemetry-end-to-end-telemetry-for-robotics/50658)
- Open-RMF: [rmf_task](https://github.com/open-rmf/rmf_task), [intermittent connectivity thread](https://discourse.openrobotics.org/t/task-execution-with-intermittent-robot-connectivity/57782.md),
  [fleet adapter changelog](https://docs.ros.org/en/jazzy/p/rmf_fleet_adapter/CHANGELOG.html)
- LeRobot: [dataset v3](https://huggingface.co/docs/lerobot/en/lerobot-dataset-v3), [porting to v3](https://huggingface.co/docs/lerobot/porting_datasets_v3),
  [HIL-SERL example (mirror)](https://huggingface.co/datasets/paulagb/hackathon-dataset_caramelos/blob/main/examples/tutorial/rl/hilserl_example.py)
- NVIDIA: [Isaac Lab-Arena](https://developer.nvidia.com/blog/simplify-generalist-robot-policy-evaluation-in-simulation-with-nvidia-isaac-lab-arena),
  [GR00T closed-loop evaluation](https://isaac-sim.github.io/IsaacLab-Arena/main/pages/example_workflows/static_apple/step_4_evaluation.html)
- OpenRAL: [ROS Discourse announcement](https://discourse.openrobotics.org/t/openral-the-agentic-harness-for-physical-ai-ros-2-native/56352),
  [Hugging Face org](https://huggingface.co/OpenRAL)
- Strands Robots: [GitHub](https://github.com/strands-labs/robots), [docs](https://strandsagents.com/docs/labs/robots/)
