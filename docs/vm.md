# Running the project in the Tart VM

Hard rule: `cargo`, `bt-probe`, the app and every check run **only inside the Tart VM `bt-vm`** (macOS guest). Never on the host, never open GUI windows on the host. `git` and `openspec` run on the host.

## 1. Starting the VM (main session / human only)

Agents never start, stop or restart the VM. The main session or a human owns it.

```bash
mkdir -p .runtime/vm
tart run bt-vm --no-graphics --net-bridged=en0 > .runtime/vm/bt-vm-run.log 2>&1
```

- `en0` is the host Wi-Fi; `tart run bt-vm --net-bridged=list` shows the available interfaces.
- Run it as a long-lived process owned by something that outlives the shell (a supervised service, `launchd`, a dedicated terminal). A `tart run ... &` from a short-lived agent shell dies with that shell and SIGTERMs the VM.
- Why bridged: with the default shared NAT the VM resolved DNS but had no outbound TCP while the host ran a full-tunnel VPN (AmneziaVPN). After a restart with the command above, outbound HTTPS worked (`curl` to `https://yandex.ru` returned 302, to `1.1.1.1` 301).
- Unconfirmed: `tart ip bt-vm` still reported `192.168.64.2` (a shared-NAT address) after that restart, so it is not known whether bridging took effect or the restart alone fixed the network. If the network breaks again, retry the same restart before digging further.

## 2. Checks

```bash
tart list | grep bt-vm                    # is it running?
tart exec bt-vm curl -sS -m 10 -o /dev/null -w '%{http_code}\n' https://yandex.ru   # network from inside; expect 200/30x
```

The VM has no `route`, `ifconfig` or `ping`; use `curl` for connectivity. If the VM is stopped, report it to the main session and wait.

## 3. Per-agent workflow

Every agent uses its own `~/bt/src-<name>`; never the shared `~/bt/src`. `tart exec` has no `~/.cargo/bin` in `PATH`, hence `$VM`.

```bash
cd <your worktree>                        # absolute path
N=<YourName>
VM="export PATH=\$HOME/.cargo/bin:\$PATH; cd ~/bt/src-$N; "

# Sync tracked + untracked, no ignored files
tart exec bt-vm sh -c "rm -rf ~/bt/src-$N && mkdir -p ~/bt/src-$N"
git ls-files -co --exclude-standard -z | xargs -0 tar -cf - | tart exec -i bt-vm sh -c "tar -xf - -C ~/bt/src-$N"

# cargo: always --locked (--offline works too)
tart exec bt-vm sh -c "$VM cargo test --workspace --locked"
tart exec bt-vm sh -c "$VM cargo clippy --workspace --all-targets --locked"
tart exec bt-vm sh -c "$VM cargo build --locked --release -p yandex-proto --bin bt-probe"
tart exec bt-vm sh -c "$VM cargo build --locked --release -p blazing-telesram"

# bt-probe; the VM session lives in ~/bt/.runtime
tart exec bt-vm sh -c "$VM BT_RUNTIME_DIR=\$HOME/bt/.runtime ./target/release/bt-probe me"
```

Remove `~/bt/src-$N` when the task is done.

## 4. Running the app and screenshots

Writes are limited to the self chat through `BT_WRITE_ONLY_CHAT`. `<self chat id>` is the account's self-chat id (not committed; ask the main session or look in the VM's `~/bt/.runtime`).

```bash
C=<self chat id>
timeout 15 tart exec bt-vm sh -c "cd ~/bt/src-$N && BT_LOG=debug BT_WRITE_ONLY_CHAT=$C BT_RUNTIME_DIR=\$HOME/bt/.runtime nohup ./target/release/blazing-telesram > ~/bt/app-$N.out 2>&1 < /dev/null &"   # rc=124 is normal
sleep 15; .runtime/vm/shot.sh NAME        # screencapture in the VM -> .runtime/vm/NAME.png
tart exec bt-vm sh -c 'f=$(ls -t ~/bt/.runtime/logs/* | head -1); tail -5 $f'
tart exec bt-vm sh -c 'pkill -f target/release/blazing-telesram'
```

Run the binary built in your own `~/bt/src-$N`. Do not use `.runtime/vm/launch.sh`: it deploys a host-built binary.

osascript helpers live in `/Users/admin/bt/*.js` inside the VM (check each script's arguments):

```bash
tart exec bt-vm osascript -l JavaScript /Users/admin/bt/click.js X Y
tart exec bt-vm osascript -l JavaScript /Users/admin/bt/rclick.js X Y            # right click
tart exec bt-vm osascript -l JavaScript /Users/admin/bt/scroll.js X Y LINES TIMES
tart exec bt-vm osascript -l JavaScript /Users/admin/bt/type.js 'текст' [return] # unicode text
tart exec bt-vm osascript -l JavaScript /Users/admin/bt/typek.js text 0.05       # key codes, a-z only
```

Offline toggle (to test offline behaviour); always turn it back on:

```bash
tart exec bt-vm sudo networksetup -setnetworkserviceenabled Ethernet off
tart exec bt-vm sudo networksetup -setnetworkserviceenabled Ethernet on
```
