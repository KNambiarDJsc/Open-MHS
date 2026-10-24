idf_component_register(SRCS "main.c"
                       INCLUDE_DIRS "."
                       REQUIRES esp_event esp_netif esp_timer driver json mqtt nvs_flash protocol_examples_common)
