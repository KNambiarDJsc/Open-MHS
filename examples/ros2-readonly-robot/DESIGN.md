# Design note: read-only robot observation through OpenMHS

Written after the repository audit and before any code. Section 1 is what the repository already
does. Section 2 is what ROS 2 does, kept apart from it, with sources. Section 3 is the design.

## 1. Audit answers

Read: `README.md`, `docs/IMPLEMENTATION_STATUS.md`, `docs/limitations.md`,
`docs/architecture/SYSTEM_DESIGN.md`, `docs/architecture/adapter-host.md`,
`docs/architecture/AUDIT_EVIDENCE.md`, ADR-0001, `spec/permission-manifest.md`,
`spec/adapter-protocol.md`, `docs/adapters/ros2.md`, `adapters/python/ros2/`, `python/openmhs/`,
`crates/openmhs-engine/src/engine.rs` (the read path), `crates/openmhs-types/src/state.rs`,
`crates/openmhs-mcp/src/lib.rs`, `examples/agent-loop/mcp_stdio.py`, `research/lab-automation/`,
`.github/workflows/ci.yml`, and the branches. `origin/feature/remote-lab-readonly-poc` is one small
OpenFlexure script, not ROS. No branch has a robot-state example.

**1. What read-only ROS 2 functionality exists?** `adapters/python/ros2/ros2_adapter.py`, an
out-of-process adapter behind the adapter host. It maps a capability to one field of one topic, keeps
the newest message per topic, and refuses a descriptor declaring anything but `read` or `subscribe`.
`execute` always raises `unsupported`. Silence longer than `stale_after_ms` reads as
`adapter.disconnected`. NaN reads as `adapter.device_failure`. CI reaches L1 against one in-process
`BatteryState` publisher, in a container.

**2. What is missing for a convincing robot-state demo?**

| Gap | Evidence | Where it would be fixed |
|---|---|---|
| No fixture beyond one battery topic, and that one runs inside the adapter, so killing the source kills the observer | `RobotSim` | Example: a separate simulated robot process |
| Joint values are addressed by array position (`position.1`) with no check of `name` | `field()` | Adapter |
| `byte` fields are `bytes` in rclpy, so `DiagnosticStatus.level` cannot be read as an integer | Confirmed: `DiagnosticStatus().level` is `b'\x00'` | Adapter |
| The adapter does not check a value against the descriptor's `range`, and the read path has no check either | `convert()`, `Engine::read` | Adapter (the Modbus adapter already does this for its own values) |
| The newest *received* message wins, even with an older stamp | `Observer._keep` | Adapter |
| A simulator the user runs in another process is labelled `source: device` | `reading(..., source=...)` | Adapter config |
| The observer node gets default parameter services from rclpy | `Node.__init__` defaults | Adapter |

These gaps are read from the code. Section 3 measures them on the unmodified adapter first, so each
change rests on an observation.

**3. Which types and contracts are reused?** The descriptor, permission manifest, adapter protocol,
`AdapterError` kinds, `reading()`, `StateValue` with `is_usable`, `ReadResult.usable`, `age_ms`,
`max_age_ms`, the MCP tools, `McpStdioClient`, `Client`, grants, scoped tokens, audit chain, and
`openmhs audit verify`.

**4. Which existing mechanisms already prove read-only behaviour?**

- Credential scopes, checked first in `Engine::read` and `submit`; refusals are audited as
  `read.denied` or a `DENIED` command.
- Grants, default deny, with `deny` winning.
- The descriptor: a capability that does not declare `write` or `invoke` cannot be commanded by anyone,
  including the operator.
- The manifest's `capabilities.allow`: the host refuses a descriptor that goes beyond it, and checks
  each call against it.
- `Settings`: the adapter refuses a descriptor with control operations at start.
- The adapter contains no publisher, service client or action client outside the conformance-only
  `RobotSim`. This is checked, not assumed (section 3).

**5. What can be built without changing the OpenMHS core?** All of it. The Rust crates need no
change. The shared change is confined to the Python ROS 2 adapter, and every addition to it is
optional or tightens behaviour.

**6. Is a core change necessary?** No.

**7. What is the smallest architecture?**

```
simulated robot process  --DDS-->  ros2_adapter.py  --stdio-->  gateway  --MCP-->  scripted agent
(publishes state; hosts            (subscribes only)            (scopes, grants,   (cited answers)
 tripwire control surfaces)                                      freshness, audit)
```

One new process (the fixture), one manifest, one agent script, one harness. No new permission layer,
freshness logic or audit path.

## 2. External facts about ROS 2 (not OpenMHS decisions)

Checked against the installed Jazzy packages (`ros2 interface show`, `inspect` on `rclpy`) where
marked *installed*, otherwise from the linked source.

- **`sensor_msgs/JointState`** is parallel arrays. "All arrays in this message should have the same
  size, or be empty. This is the only way to uniquely associate the joint name with the correct
  states." Each field may be empty. So position in the array is not identity, and an empty `position`
  is legal. *installed*
- **`nav_msgs/Odometry`**: the pose is in `header.frame_id`, the twist in `child_frame_id`. *installed*
- **`odom` is not a global position.** REP 105: `odom` is world-fixed and continuous, but its error is
  unbounded, and it can drift. `map` should not drift but can jump.
  [REP 105](https://ros.org/reps/rep-0105.html)
- **`sensor_msgs/BatteryState`**: NaN means "unmeasured". Percentage is on a 0 to 1 range. *installed*
- **`diagnostic_msgs/DiagnosticStatus`**: `level` is a `byte` (OK 0, WARN 1, ERROR 2, STALE 3), which rclpy
  gives as `bytes`. Values are string key and value pairs. *installed*
- **TF** is a tree assembled over time, not a table of values. A `TFMessage` is an array of stamped
  edges, and any message may carry any subset. Consumers use a buffer that interpolates and raises lookup,
  connectivity and extrapolation errors. `/tf_static` is published with transient-local durability.
  [tf2_ros](https://docs.ros.org/en/kilted/p/tf2_ros/_sources/doc/index.rst.txt),
  [transforms troubleshooting](https://docs.picknik.ai/troubleshooting/transforms_troubleshooting/)
- **QoS.** A best-effort subscription matches both reliable and best-effort publishers; a reliable
  subscription does not match a best-effort publisher, and a mismatch gives no data and, by default, no
  error. [QoS](https://docs.ros.org/en/jazzy/Concepts/Intermediate/About-Quality-of-Service-Settings.html),
  [Clearpath](https://docs.clearpathrobotics.com/docs/ros/api/overview)
- **Clocks.** With `use_sim_time`, ROS time follows `/clock` and can jump backwards, for example on bag
  playback. [Clock and Time](https://design.ros2.org/articles/clock_and_time.html)
- **rclpy nodes** start parameter services and a rosout publisher unless told not to
  (`start_parameter_services`, `enable_rosout`). *installed*

## 3. Decisions

**Measure the gaps first.** Before changing the adapter, the fixture runs against the unmodified
adapter and the results are kept in `results/baseline_unmodified_adapter/`.

**Adapter changes, each only if the baseline confirms the gap:**

1. A path selector for named things: `position[name=elbow]` and `status[name=driver].level`. Position
   in an array is not identity in these messages.
2. A single-`byte` field becomes an integer.
3. A value outside the descriptor's `range` is `malformed_response`.
4. An older message never replaces a newer one on the same topic. Repeated older stamps surface as a
   failure that says so.
5. `config.simulated_source`, declaring that the topics come from a simulator the user runs. It is a
   declaration: nothing verifies it.
6. The observer node starts without parameter services and without a rosout publisher.

**Not flattened:**

- **TF is not exposed at all.** A flat capability would need a position in `transforms`, which is not
  stable, and it would skip the composition and time lookup that give TF its meaning. Odometry gives
  the pose with its frame stated.
- **Pose is several reads.** Descriptor values are scalars and each read is a separate call, so `x`, `y`
  and the quaternion components may come from different messages. The agent reports the spread of their
  stamps instead of presenting them as one sample. Fixing it needs a structured value type, which is a
  core change this evaluation does not justify.
- **`DiagnosticStatus.values`** are strings, and they stay out: parsing them would turn device text
  into numbers.
- **Odometry is reported as odometry**, with its frame, never as "where the robot is on the map".

**Freshness has one source.** The agent uses the gateway's `usable`, `age_ms` and `max_age_ms`, and the
adapter's silence rule. It adds no threshold of its own.

**Control surfaces exist so that zero means something.** The simulated robot hosts a `/cmd_vel`
subscriber, a joint-trajectory subscriber, a mode service, an action server, and a settable parameter.
Each counts what reaches it. A positive control pokes each one directly from a separate ROS node and
must see the count rise; without that, a count of zero could just mean a broken counter.

**Free text is not granted.** The agent may read `diagnostics.driver.level` but not
`diagnostics.driver.message`. Device-supplied text is an injection surface, and this is also the
"ungranted capability" of scenario E.

**One Linux host, one clock** (section 4). The pinned container path in `docs/adapters/ros2.md` is not used
by this example, because the fixture must be a separate killable process on the same DDS domain.

**No core change, no second permission mechanism, no remote deployment.** Everything is on one host, with
`ROS_AUTOMATIC_DISCOVERY_RANGE=LOCALHOST` and its own `ROS_DOMAIN_ID`.

## 4. What the baseline and the first runs changed

**The baseline confirmed the gaps** (`results/baseline_unmodified_adapter/baseline.json`, adapter at `a5d8d94`):

| Observed on the unmodified adapter | Result |
|---|---|
| Joint array reversed, read `position.0` as the shoulder | Returned the **wrist's** 0.3 rad, `GOOD`, usable |
| Elbow publishes 1000 rad against a declared range of 6.3 | Returned 1000 rad, `GOOD`, usable |
| Stamps alternating 200 ms back, 200 reads | 19 reads went back in time, all usable |
| `DiagnosticStatus.level` | Always `adapter.malformed_response`: `bytes`, not an integer |
| A simulator in another process | `source: device` |
| The observer node in the ROS graph | Seven parameter and type services, including `set_parameters`, plus `/rosout` |

The first two are the ones that matter: wrong data, labelled good. Reading the code had also suggested that a
message with no stamp was given the time it arrived. `docs/adapters/ros2.md` said so; the code gave the time of
the read, so a stale unstamped value looked new at each read until silence caught it. That is fixed.

**The plan to run a Windows gateway against ROS 2 in WSL was dropped.** A first version launched the adapter
through `wsl.exe`. Measured over 27 s, Windows' clock minus the distro's clock went from +421 ms to -258 ms to
+1135 ms. The gateway judges freshness from stamps against its own clock, and the limit here is 1000 ms, so that
setup measures two clocks disagreeing, not the robot. Everything now runs in Linux on one clock, and the launcher
code is gone. This is the requirement `docs/adapters/ros2.md` already states (synchronized clocks), seen to matter.

**The WSL2 wall clock itself steps back about 1.7 s every 30 s or so**, even idle (measured: -1706 ms at 28.8 s and
-1682 ms at 61.1 s; `hv_utils` time sync on an otherwise fast guest clock). On a real host, NTP steps are rarer. Two
consequences, both kept in view rather than worked around: the evaluation records every step, and such a step is
a real test of rule 4 below, because stamps really do go backwards.

**A rule from section 3 had a cost.** "An older message never replaces a newer one" means a clock stepping back by
D blinds a topic for about D, and reads fail (`adapter.malformed_response`, naming the reason) once that exceeds
`stale_after_ms`. A large step, or a looping bag with `use_sim_time`, blinds it until stamps catch up or the adapter
restarts. This was kept because the alternative is silently returning older data as current, and the failure is
visible. It is a limitation for anyone on a clock that steps; see the README.

**An attempt that proved nothing was caught.** The first trial's control attempts were malformed and refused as
`command.schema`, which counted as "refused" and said nothing about permissions. `command_attempt` now raises on a
malformed request, and the refusals carry the codes that matter: `auth.scope.denied`, `policy.denied.explicit`,
`policy.denied.no_grant`, `command.operation.unsupported`.
