# Run a node on a Windows workstation

Platform-Version: 0.1.0
Drill: scripts/verify/node.sh
Applies-To: a Windows 11 machine that runs the platform before a server is available, and after any
            change to its WSL configuration

**Why:** a node is the same on every Linux host: the units generated from the manifest, the paths
the manifest names, systemd and a delegated cgroup v2 subtree. WSL 2 with systemd enabled is such a
host. Nothing in the platform changes for it, so a node rehearsed here moves to a server with
[Move a node to another host](move_a_node_to_another_host.md) and runs from the same units.

## Precondition

- Windows 11 with WSL 2 and an Ubuntu distribution. Measured on WSL 2.7.10, kernel 6.18 and Ubuntu
  systemd 255 (2026-11-27). The platform needs systemd 250 or newer.
- The platform built from source inside the distribution, not on a Windows drive.

## Procedure

1. Enable systemd as the distribution's init. In `/etc/wsl.conf`:

   ```
   [boot]
   systemd=true
   ```

2. Keep the distribution running when no terminal is open. In `%UserProfile%\.wslconfig`:

   ```
   [general]
   instanceIdleTimeout=-1
   ```

   Without it WSL stops an idle distribution after 15 seconds (the documented default), and the
   node stops with it.

3. Restart WSL from Windows with `wsl --shutdown`, then open the distribution again.
4. Wait until `systemctl is-system-running` answers `running`. On the first start after step 1 it
   answered `initializing` for 3 minutes while `systemd-tmpfiles-setup` ran (measured 2026-11-27).
5. Check the clock: `timedatectl` must show `System clock synchronized: yes`. Venue events are
   ordered by venue time, but the clock gate and the readiness documents read the host clock.
6. Rehearse the install with `bash scripts/verify/node.sh`. It must end with `CLEAN`.
7. Install and start the node with [Install a node and bring it up](install_and_start_a_node.md).

Before the Strategy Engine is enabled, prove the run bound on this host with
[Verify an agent run can be bounded](verify_an_agent_run_can_be_bounded.md). That drill runs under
the user manager, which must run without a login session: `sudo loginctl enable-linger <user>`.

## Reading the result

| Check                          | Expected                             |
|--------------------------------|--------------------------------------|
| `ps -p 1 -o comm=`             | `systemd`                            |
| `systemctl is-system-running`  | `running`                            |
| `bash scripts/verify/node.sh`  | seven `PASS` lines, then `CLEAN`     |
| The bound drill                | three `PASS` lines                   |

All four held on the host measured above (2026-11-27).

## What this host does not give

- **Uptime.** The node runs while Windows runs. Sleep, hibernation, a restart for an update or a
  `wsl --shutdown` stop it, and nothing starts the distribution again after Windows starts until a
  user opens it. A node that holds positions needs
  [Close positions when a node stops](close_positions_when_a_node_stops.md) in place before it
  trades.
- **A server's network path.** WSL uses NAT by default. Venue latency measured here is the
  workstation's, so [Measure what an exchange costs on this node](measure_what_an_exchange_costs.md)
  is run again after a move.
