# Home Assistant and HomeKit

The daemon can publish itself to Home Assistant over MQTT discovery. Home Assistant then exposes the TV's ambient sync as one device, and its **HomeKit Bridge** integration can pass the switches on to Apple Home and Siri.

```text
Apple Home / Siri  ⇄  HA HomeKit Bridge  ⇄  Home Assistant  ⇄  MQTT broker  ⇄  lg-hue-sync (on the TV)
```

The bridge is a client of the daemon's own loopback HTTP API (`127.0.0.1:8088`), so every command gets the same validation as the dashboard. It is off unless configured, and a missing broker never affects light output.

## Setup

1. Run an MQTT broker that Home Assistant also uses (for example the Mosquitto add-on) and enable the HA MQTT integration.
2. Add an `mqtt` section to `/var/home/root/lg-hue-sync/config.json` on the TV:

   ```json
   "mqtt": {
     "host": "192.168.1.10",
     "port": 1883,
     "username": "lg-hue-sync",
     "password": "…"
   }
   ```

   Optional fields: `enabled` (default `true`), `base_topic` (default `lg-hue-sync`), `discovery_prefix` (default `homeassistant`). Plain TCP only; keep the broker on your LAN. The password is stored in this file like the Hue and Nanoleaf credentials, so never commit or copy a populated config.

3. Restart the service so it reads the new section: `ssh root@<tv-ip> 'systemctl restart lg-hue-sync'`. (The dashboard's restart control only restarts the capture pipeline, not the process.) Home Assistant should show a device named **LG TV Ambient Sync** within a few seconds. The log line `MQTT connected` confirms the broker link; an invalid section logs `MQTT bridge disabled: …` and the daemon carries on.

Dashboard-style persistence applies: changes made from Home Assistant are **live until saved**. They do not rewrite `config.json`; Save from the dashboard to keep a tuning across restarts.

## Entities

| Entity | Type | Notes |
| --- | --- | --- |
| Screen sync | switch | Start/stop capture and output (`/api/start`, `/api/stop`) |
| Hue sync | switch | Release or reclaim the Hue entertainment area |
| Nanoleaf sync | switch | Stop or resume Nanoleaf streaming |
| Display | switch | Panel on/off without TV standby; unknown while the TV is in standby |
| Strobe boost | switch | Flash the lights while the video strobes (Instant turns it on) |
| Colour style | select | Soft / Default / Vivid presets |
| Response | select | Slow / Default / Fast / Instant / Dynamic. Instant applies the full low-latency bundle (no hold, instant cuts, strobe boost); Dynamic scales between gentle and Instant with on-screen energy |
| Spatial blending | select | Local / Balanced / Soft |
| Brightness, Hue brightness, Nanoleaf brightness, Saturation, Gamma | number | Same ranges as the dashboard |
| Hue connected, Nanoleaf connected | binary sensor | Diagnostic |
| Screen energy | sensor | 0–100 % on-screen motion/flash energy that drives Dynamic |
| Pipeline state, TV power state, Light update rate | sensor | Diagnostic |

Values the daemon cannot map to a label (a preset or response curve set from the dashboard that is not one of the options above) show as *unknown* rather than a wrong value.

## HomeKit

HomeKit Bridge exposes entity domains HomeKit understands. The four **switches** map directly to Home accessories: "Hey Siri, turn on Screen sync". Selects and numbers are Home Assistant concepts, so to use them from Apple Home wrap them in scripts or scenes and expose those:

```yaml
# configuration.yaml
homekit:
  - name: TV Ambient
    port: 21064
    filter:
      include_entities:
        - switch.lg_tv_ambient_sync_screen_sync
        - switch.lg_tv_ambient_sync_hue_sync
        - switch.lg_tv_ambient_sync_nanoleaf_sync
        - script.tv_ambient_vivid

script:
  tv_ambient_vivid:
    alias: TV Ambient Vivid
    sequence:
      - action: select.select_option
        target: { entity_id: select.lg_tv_ambient_sync_colour_style }
        data: { option: Vivid }
```

Entity ids follow the device name; check yours under *Settings → Devices*. Apple's native TV accessory from the LG firmware is independent and unaffected. Which entity domains HomeKit Bridge supports changes between Home Assistant releases, so check its documentation for your version.

## Without MQTT

Everything above is also available over the HTTP API (`GET /openapi.json` lists it), which Home Assistant can call with `rest_command` or the RESTful integration:

```yaml
rest_command:
  tv_ambient_start:
    url: "http://<tv-ip>:8088/api/start"
    method: post
```

The HTTP API has no authentication. Keep it on a trusted LAN and do not forward port 8088.

## Topics

With the default base topic: `lg-hue-sync/availability` (`online`/`offline`, last-will), `lg-hue-sync/state` (one retained JSON document) and `lg-hue-sync/<entity>/set` for commands. Entities and discovery payloads are defined in `src/mqtt.rs`.
