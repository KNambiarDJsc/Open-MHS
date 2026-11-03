# Layer 1: the same questions on a real rclpy action server

Layer 1a tests the premises of the Layer 0 model against a real `rclpy` action server. Results are in
`../LAYER1A_RESULTS.md`. Plan and refutation conditions are in `../LAYER1_PLAN.md`.

## Environment used

ROS 2 Jazzy (`ros-jazzy-ros-base`, `ros-jazzy-example-interfaces`) on Ubuntu 24.04 under WSL2, in a
dedicated distro so nothing else is modified. `packages.ros.org` served a TLS certificate for a different
host (`*.osuosl.org`) when this was set up (2026-11-03), so apt used the signed HTTP repository and the signing
key was fetched over HTTPS from GitHub (`ros/rosdistro`, key fingerprint
`C1CF6E31E6BADE8868B172B4F42ED6FBAB17C654`). Do not disable certificate checks.

```
wsl --export Ubuntu ubuntu-base.tar
wsl --import openmhs-ros <dir> ubuntu-base.tar --version 2
# inside openmhs-ros, as root: add the ROS apt source, apt install ros-jazzy-ros-base ros-jazzy-example-interfaces
```

Remove it with `wsl --unregister openmhs-ros`.

## Run

```
source /opt/ros/jazzy/setup.bash
export ROS_DOMAIN_ID=77 ROS_LOCALHOST_ONLY=1
python3 probe.py --n 20 --out ../results/layer1a
python3 -I analyze.py ../results/layer1a
```

Each trial runs in its own process. The effect ledger and the server event log are written to
`<out>/work/` so the ground truth is independent of the client. `server.py` takes `--result-timeout`;
use 10 to match the rcl_action default that rclcpp servers inherit (the rclpy default is 900).

## Known limits

One RMW, one machine, localhost only; `example_interfaces/Fibonacci` is a carrier (`order` is the operation
id); the kill is a SIGKILL of the server process, not a network fault.
