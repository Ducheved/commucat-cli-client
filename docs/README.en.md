# CommuCat CLI Client

An interactive CCP-1 console client with a k9s-inspired interface. The left pane lists channels, the right pane shows message history, and the footer is used for entering commands or chat messages.

## Prepare a profile
1. Generate device keys and the local profile:
   ```bash
   commucat-cli-client init \
     --server https://example.org:8443 \
     --domain example.org
   ```
   Optional flags: `--pattern`, `--prologue`, `--tls-ca`, `--server-static`, `--traceparent`.
2. Register the printed device on the server:
   ```bash
   commucat-cli rotate-keys <device_id>
   ```
3. The profile is stored at `~/.config/commucat/client.json`. Set `COMMUCAT_CLIENT_HOME` to override the location.

## Start the client
```
commucat-cli-client tui
```
Key bindings:
- `:` — enter command mode.
- `Tab` — cycle channels.
- `Esc` — clear the input buffer.
- `Ctrl+C` or `F10` — exit.

## Commands (prefixed with `:`)
- `connect` — establish a CCP-1 tunnel.
- `disconnect` — close the active session.
- `join <channel_id> <member1,member2,...>` — announce membership for a channel.
- `relay <channel_id> <member1,member2,...>` — join with relay fan-out enforced.
- `leave <channel_id>` — leave a channel.
- `channel <channel_id>` — switch the active pane.
- `presence <state>` — publish a Presence frame, e.g. `presence away`.
- `export` — print device keys.
- `clear` — wipe message buffers.
- `help` — short cheat sheet.
- `quit` — exit the client.

Type plain text (without `:`) to send messages to the active channel. The message view marks inbound frames with `<`, outbound with `>`, and system entries with `*`. Each channel keeps up to 200 entries.

## TLS hints
- Provide a custom CA bundle: `--tls-ca /path/to/root.pem`.
- Test-only shortcut: `--insecure` to skip certificate validation.

## Troubleshooting
- Ensure the device is present in PostgreSQL `user_device` and matches the Noise static key.
- Confirm that the front-end terminator supports TLS 1.3 and HTTP/2.
- Enable verbose logs: `RUST_LOG=debug commucat-cli-client tui`.

## Quick walkthrough
```bash
# profile setup
commucat-cli-client init --server https://relay.local:8443 --domain relay.local --insecure --force

# register on the server
commucat-cli rotate-keys device-1730000000000

# interactive session
commucat-cli-client tui
:connect
:join 42 device-1730000000000,peer@example.org
hello, world!
```
