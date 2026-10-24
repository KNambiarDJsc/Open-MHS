/*
 * An ESP32 node that speaks the OpenMHS MQTT conventions (docs/adapters/mqtt.md):
 *
 *   <prefix>/state   retained JSON, QoS 1, every second and after each command
 *   <prefix>/set     {"led": true|false, "id": "..."}   drive the LED output
 *   <prefix>/pulse   {"n": 1..10, "id": "..."}          count n pulses (not idempotent)
 *   <prefix>/ack     {"id": "...", "ok": true|false, "error": "..."}  one per command
 *
 * The clock comes from SNTP, or, with NODE_CLOCK_FROM_MQTT (used under QEMU, where the
 * host's UDP port 123 is often taken), from <prefix>/clock {"epoch_ms": N}, which the node
 * asks for by publishing <prefix>/hello when it connects. State is not published until the
 * clock is set, so every state message carries the node's own time.
 *
 * It is written to run under Espressif's QEMU (no hardware): the network is the emulated
 * OpenCores Ethernet MAC, the broker and the SNTP server are the host, 10.0.2.2. The same
 * source builds for a board by switching protocol_examples_common to Wi-Fi in menuconfig.
 * The LED output is a GPIO level; under QEMU nothing is attached to it.
 *
 * A message that is not JSON, or has no string "id", is ignored: with no id there is no
 * one to acknowledge to.
 */

#include <stdbool.h>
#include <stdio.h>
#include <math.h>
#include <string.h>
#include <sys/time.h>
#include <time.h>

#include "cJSON.h"
#include "driver/gpio.h"
#include "esp_event.h"
#include "esp_log.h"
#include "esp_netif.h"
#include "esp_netif_sntp.h"
#include "esp_timer.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "mqtt_client.h"
#include "nvs_flash.h"
#include "protocol_examples_common.h"

static const char *TAG = "node";

#define PREFIX CONFIG_NODE_TOPIC_PREFIX
#define TOPIC_STATE PREFIX "/state"
#define TOPIC_SET PREFIX "/set"
#define TOPIC_PULSE PREFIX "/pulse"
#define TOPIC_ACK PREFIX "/ack"
#define TOPIC_CLOCK PREFIX "/clock"
#define TOPIC_HELLO PREFIX "/hello"

static esp_mqtt_client_handle_t client;
static volatile bool connected = false;
static bool led = false;
static uint32_t pulses = 0;
static bool clock_synced = false;
static SemaphoreHandle_t lock;

/* RFC 3339 with milliseconds, or "" if the clock was never set. */
static size_t now_rfc3339(char *out, size_t len)
{
    struct timeval tv;
    gettimeofday(&tv, NULL);
    if (!clock_synced) {
        out[0] = 0;
        return 0;
    }
    struct tm tm;
    gmtime_r(&tv.tv_sec, &tm);
    size_t n = strftime(out, len, "%Y-%m-%dT%H:%M:%S", &tm);
    n += snprintf(out + n, len - n, ".%03dZ", (int)(tv.tv_usec / 1000));
    return n;
}

static void publish_state(void)
{
    if (!connected || (CONFIG_NODE_CLOCK_FROM_MQTT && !clock_synced)) {
        return;
    }
    char ts[40];
    char buf[200];
    now_rfc3339(ts, sizeof ts);
    xSemaphoreTake(lock, portMAX_DELAY);
    int n = snprintf(buf, sizeof buf, "{\"led\":%s,\"pulses\":%u,\"uptime_s\":%.1f", led ? "true" : "false",
                     (unsigned)pulses, esp_timer_get_time() / 1e6);
    if (ts[0]) {
        n += snprintf(buf + n, sizeof buf - n, ",\"time\":\"%s\"", ts);
    }
    n += snprintf(buf + n, sizeof buf - n, "}");
    xSemaphoreGive(lock);
    esp_mqtt_client_publish(client, TOPIC_STATE, buf, n, 1, 1);
}

static void ack(const char *id, bool ok, const char *error)
{
    cJSON *o = cJSON_CreateObject();
    cJSON_AddStringToObject(o, "id", id);
    cJSON_AddBoolToObject(o, "ok", ok);
    if (error) {
        cJSON_AddStringToObject(o, "error", error);
    }
    char *s = cJSON_PrintUnformatted(o);
    if (s) {
        esp_mqtt_client_publish(client, TOPIC_ACK, s, 0, 0, 0);
        cJSON_free(s);
    }
    cJSON_Delete(o);
}

static void handle_set(cJSON *msg, const char *id)
{
    const cJSON *v = cJSON_GetObjectItemCaseSensitive(msg, "led");
    if (!cJSON_IsBool(v)) {
        ack(id, false, "led must be true or false");
        return;
    }
    xSemaphoreTake(lock, portMAX_DELAY);
    led = cJSON_IsTrue(v);
    gpio_set_level(CONFIG_NODE_LED_GPIO, led);
    xSemaphoreGive(lock);
    ESP_LOGI(TAG, "led -> %s (command %s)", led ? "on" : "off", id);
    ack(id, true, NULL);
    publish_state();
}

static void handle_pulse(cJSON *msg, const char *id)
{
    const cJSON *v = cJSON_GetObjectItemCaseSensitive(msg, "n");
    if (!cJSON_IsNumber(v) || v->valuedouble != (int)v->valuedouble || v->valueint < 1 || v->valueint > 10) {
        ack(id, false, "n must be an integer from 1 to 10");
        return;
    }
    xSemaphoreTake(lock, portMAX_DELAY);
    pulses += v->valueint;
    xSemaphoreGive(lock);
    ESP_LOGI(TAG, "pulses += %d (command %s)", v->valueint, id);
    ack(id, true, NULL);
    publish_state();
}

static void handle_clock(cJSON *msg)
{
    const cJSON *ms = cJSON_GetObjectItemCaseSensitive(msg, "epoch_ms");
    if (!cJSON_IsNumber(ms) || ms->valuedouble < 1.7e12) {
        return;
    }
    struct timeval now;
    gettimeofday(&now, NULL);
    double have_ms = (double)now.tv_sec * 1000.0 + now.tv_usec / 1000.0;
    /* Set the clock the first time, and step it later only if the source disagrees by more
     * than a second (a node that was stalled lost time). */
    if (clock_synced && fabs(have_ms - ms->valuedouble) < 1000.0) {
        return;
    }
    struct timeval tv = {.tv_sec = (time_t)(ms->valuedouble / 1000), .tv_usec = (int)(fmod(ms->valuedouble, 1000) * 1000)};
    settimeofday(&tv, NULL);
    clock_synced = true;
    ESP_LOGI(TAG, "clock set from the broker");
    publish_state();
}

static void on_message(const char *topic, int topic_len, const char *data, int data_len)
{
    char *copy = strndup(data, data_len);
    if (!copy) {
        return;
    }
    cJSON *msg = cJSON_Parse(copy);
    free(copy);
    if (msg && topic_len == (int)strlen(TOPIC_CLOCK) && !strncmp(topic, TOPIC_CLOCK, topic_len)) {
        handle_clock(msg);
        cJSON_Delete(msg);
        return;
    }
    const cJSON *id = msg ? cJSON_GetObjectItemCaseSensitive(msg, "id") : NULL;
    if (!cJSON_IsString(id) || !id->valuestring) {
        ESP_LOGW(TAG, "ignoring a message with no usable id on %.*s", topic_len, topic);
    } else if (topic_len == (int)strlen(TOPIC_SET) && !strncmp(topic, TOPIC_SET, topic_len)) {
        handle_set(msg, id->valuestring);
    } else if (topic_len == (int)strlen(TOPIC_PULSE) && !strncmp(topic, TOPIC_PULSE, topic_len)) {
        handle_pulse(msg, id->valuestring);
    }
    cJSON_Delete(msg);
}

static void on_mqtt(void *arg, esp_event_base_t base, int32_t event_id, void *event_data)
{
    esp_mqtt_event_handle_t e = event_data;
    switch ((esp_mqtt_event_id_t)event_id) {
    case MQTT_EVENT_CONNECTED:
        ESP_LOGI(TAG, "connected to the broker");
        connected = true;
        esp_mqtt_client_subscribe(client, TOPIC_SET, 1);
        esp_mqtt_client_subscribe(client, TOPIC_PULSE, 1);
        if (CONFIG_NODE_CLOCK_FROM_MQTT) {
            esp_mqtt_client_subscribe(client, TOPIC_CLOCK, 1);
            esp_mqtt_client_publish(client, TOPIC_HELLO, "{\"boot\":true}", 0, 0, 0);
        }
        publish_state();
        break;
    case MQTT_EVENT_DISCONNECTED:
        ESP_LOGW(TAG, "disconnected from the broker");
        connected = false;
        break;
    case MQTT_EVENT_DATA:
        if (e->data_len == e->total_data_len && e->topic) {
            on_message(e->topic, e->topic_len, e->data, e->data_len);
        } else {
            ESP_LOGW(TAG, "ignoring a fragmented message");
        }
        break;
    default:
        break;
    }
}

static void state_task(void *arg)
{
    for (;;) {
        publish_state();
        vTaskDelay(pdMS_TO_TICKS(1000));
    }
}

void app_main(void)
{
    ESP_LOGI(TAG, "OpenMHS ESP32 MQTT node, topics under %s", PREFIX);
    lock = xSemaphoreCreateMutex();
    ESP_ERROR_CHECK(nvs_flash_init());
    ESP_ERROR_CHECK(esp_netif_init());
    ESP_ERROR_CHECK(esp_event_loop_create_default());

    gpio_reset_pin(CONFIG_NODE_LED_GPIO);
    gpio_set_direction(CONFIG_NODE_LED_GPIO, GPIO_MODE_OUTPUT);
    gpio_set_level(CONFIG_NODE_LED_GPIO, 0);

    ESP_ERROR_CHECK(example_connect());

#if !CONFIG_NODE_CLOCK_FROM_MQTT
    esp_sntp_config_t sntp = ESP_NETIF_SNTP_DEFAULT_CONFIG(CONFIG_NODE_SNTP_SERVER);
    ESP_ERROR_CHECK(esp_netif_sntp_init(&sntp));
    if (esp_netif_sntp_sync_wait(pdMS_TO_TICKS(20000)) == ESP_OK) {
        clock_synced = true;
        ESP_LOGI(TAG, "clock set from %s", CONFIG_NODE_SNTP_SERVER);
    } else {
        ESP_LOGW(TAG, "no SNTP answer from %s: messages carry no timestamp", CONFIG_NODE_SNTP_SERVER);
    }
#endif

    esp_mqtt_client_config_t cfg = {
        .broker.address.uri = CONFIG_NODE_BROKER_URI,
        .credentials.client_id = "openmhs-esp32-node-01",
        .session.keepalive = 5,
        .network.reconnect_timeout_ms = 1000,
    };
    client = esp_mqtt_client_init(&cfg);
    esp_mqtt_client_register_event(client, ESP_EVENT_ANY_ID, on_mqtt, NULL);
    esp_mqtt_client_start(client);
    xTaskCreate(state_task, "state", 4096, NULL, 5, NULL);
}
