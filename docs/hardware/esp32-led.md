# ESP32 LED controller over HTTP

> **Unverified.** This guide and its sketch were written to match the example profile
> [led-controller.json](../../adapters/openmhs-http/profiles/led-controller.json). The
> project has **not compiled, flashed or run** this sketch. The OpenMHS side (the HTTP
> adapter and that profile) is tested only against the in-repo simulator. Read
> [README.md](README.md) first.

An ESP32 development board drives one LED and serves the small JSON API the profile
describes. Through the gateway, an agent can read the LED and the chip's temperature,
dim the LED, and blink it, subject to policy.

Risk: an LED and a resistor on USB power. Nothing here switches mains, motors or
heaters. Do not adapt it to such loads without independent hardware protection.

## You need

- An ESP32 board. The temperature reading uses the chip's internal sensor
  (`temperatureRead()`); on the original ESP32 that value is rough. It is the chip's
  temperature, not the room's.
- One LED and a 220–330 Ω resistor, or the board's built-in LED.
- Arduino IDE 2 with the **esp32** board package 3.x, and the **ArduinoJson** library 7.x.
- The board and the computer running OpenMHS on the same network.

## Wiring

```text
GPIO 2 ── 220 Ω ── LED anode (long leg)
GND ───────────── LED cathode (short leg)
```

Many boards already have an LED on GPIO 2. Change `LED_PIN` if yours differs.

## The API the board serves

| Request | Body | Answer |
|---|---|---|
| `GET /api/state` | — | `{"led": {"on": true, "brightness": 40}, "sensor": {"temperature": 41.5}, "time": 1761300000000}` |
| `PUT /api/led` | `{"on": true}` or `{"brightness": 40}` | `{"ok": true}`, or `409 {"ok": false, "error": "…"}` |
| `POST /api/blink` | `{"count": 3, "id": "<command id>"}` | `{"ok": true}`, or `409` when the LED is off |

`time` is when the board measured, in Unix milliseconds. It needs the board's clock set
over NTP. Until the clock is set, the sketch leaves `time` out, and the gateway reports
reads as malformed rather than guess an age. Without internet access, remove
`"timestamp_pointer": "/time"` from the profile: the time of arrival is then used.

## The sketch

```cpp
// OpenMHS example: ESP32 LED controller (docs/hardware/esp32-led.md). UNVERIFIED.
#include <WiFi.h>
#include <WebServer.h>
#include <ArduinoJson.h>
#include <sys/time.h>
#include "time.h"

const char* WIFI_SSID = "your-network";      // do not commit real credentials
const char* WIFI_PASSWORD = "your-password";
const int LED_PIN = 2;

WebServer server(80);
bool ledOn = false;
int brightness = 40;          // percent
int blinksLeft = 0;           // half-periods still to run
unsigned long nextToggle = 0;
bool blinkPhase = false;

void applyLed() {
  bool lit = ledOn && !(blinksLeft > 0 && blinkPhase);
  ledcWrite(LED_PIN, lit ? map(brightness, 0, 100, 0, 255) : 0);
}

bool clockSet() { return time(nullptr) > 1700000000; }

int64_t nowMillis() {
  struct timeval tv;
  gettimeofday(&tv, nullptr);
  return (int64_t)tv.tv_sec * 1000 + tv.tv_usec / 1000;
}

void reply(int status, JsonDocument& doc) {
  String out;
  serializeJson(doc, out);
  server.send(status, "application/json", out);
}

void refuse(int status, const char* why) {
  JsonDocument doc;
  doc["ok"] = false;
  doc["error"] = why;
  reply(status, doc);
}

void ok() {
  JsonDocument doc;
  doc["ok"] = true;
  reply(200, doc);
}

void handleState() {
  JsonDocument doc;
  doc["led"]["on"] = ledOn;
  doc["led"]["brightness"] = brightness;
  doc["sensor"]["temperature"] = round(temperatureRead() * 10) / 10.0;
  if (clockSet()) doc["time"] = nowMillis();
  reply(200, doc);
}

void handleLed() {
  JsonDocument body;
  if (deserializeJson(body, server.arg("plain"))) return refuse(400, "the body is not JSON");
  if (body["on"].is<bool>()) {
    ledOn = body["on"].as<bool>();
  } else if (body["brightness"].is<float>()) {
    float b = body["brightness"].as<float>();
    if (b < 0 || b > 100) return refuse(409, "brightness must be 0..100");
    brightness = (int)round(b);
  } else {
    return refuse(400, "expected 'on' or 'brightness'");
  }
  applyLed();
  ok();
}

void handleBlink() {
  JsonDocument body;
  if (deserializeJson(body, server.arg("plain"))) return refuse(400, "the body is not JSON");
  if (!body["count"].is<int>()) return refuse(400, "expected an integer 'count'");
  int count = body["count"].as<int>();
  if (count < 1 || count > 10) return refuse(409, "count must be 1..10");
  if (!ledOn) return refuse(409, "the LED is off");
  // Answer now and blink from loop(): the gateway waits at most timeout_ms (500 ms).
  blinksLeft = count * 2;
  blinkPhase = false;
  nextToggle = millis();
  ok();
}

void setup() {
  Serial.begin(115200);
  ledcAttach(LED_PIN, 5000, 8);   // esp32 core 3.x
  applyLed();
  WiFi.begin(WIFI_SSID, WIFI_PASSWORD);
  while (WiFi.status() != WL_CONNECTED) delay(250);
  Serial.print("OpenMHS LED controller at http://");
  Serial.println(WiFi.localIP());
  configTime(0, 0, "pool.ntp.org");
  server.on("/api/state", HTTP_GET, handleState);
  server.on("/api/led", HTTP_PUT, handleLed);
  server.on("/api/blink", HTTP_POST, handleBlink);
  server.begin();
}

void loop() {
  server.handleClient();
  if (blinksLeft > 0 && (long)(millis() - nextToggle) >= 0) {
    blinkPhase = !blinkPhase;
    blinksLeft--;
    nextToggle = millis() + 200;
    applyLed();
  }
}
```

The firmware enforces its own limits (brightness range, blink count, no blinking while
off) and answers `409` with a reason. The gateway reports that as `REJECTED`. Policy in
the gateway is a second layer, not a replacement for limits in the device.

## Connect it to OpenMHS

1. Flash the sketch and open the serial monitor at 115200 baud. Note the address it
   prints.
2. Check the board directly:

   ```sh
   curl http://192.168.1.60/api/state
   ```

3. Copy the profile and point it at the board:

   ```sh
   cp adapters/openmhs-http/profiles/led-controller.json my-led.json
   # edit my-led.json: "base_url": "http://192.168.1.60"
   openmhs profile check my-led.json
   openmhs conformance --profile my-led.json   # against the simulator, not the board
   ```

4. Serve it and try it:

   ```sh
   openmhs serve --profile my-led.json                      # terminal 1
   openmhs grants add --id led-read --principal operator:local --device http.led-01 \
     --capability '*' --ops read --effect allow             # terminal 2
   openmhs read http.led-01 sensor.temperature
   openmhs grants add --id led-write --principal operator:local --device http.led-01 \
     --capability 'led.*' --ops write,invoke --effect allow
   openmhs exec http.led-01 led.on --value true
   openmhs exec http.led-01 led.brightness --value 80%
   openmhs exec http.led-01 led.blink --param count=3
   ```

   Each `exec` prints a receipt. Writes are confirmed by reading the board back. The
   blink ends `UNVERIFIABLE`, because the board reports no blink state.

5. For an AI agent, create an agent token and grants as in [cli.md](../cli.md), and
   connect it with `openmhs mcp` ([mcp.md](../mcp.md)).

## If something does not work

| Symptom | Likely cause |
|---|---|
| `adapter.disconnected` on start | wrong `base_url`, the board is not on the network, or a firewall |
| reads fail with "no timestamp at '/time'" | the board's clock is not set yet (no NTP); wait, or remove `timestamp_pointer` |
| readings flagged stale | the board's clock is wrong; check NTP |
| `adapter.malformed_response` | the board answered something other than the JSON above; `curl` it to see |
| a command ends `UNKNOWN_OUTCOME` | the board did not answer within 500 ms; raise `timeout_ms`, and check that handlers do not block |
