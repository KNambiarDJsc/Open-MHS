cmake_minimum_required(VERSION 3.16)

# protocol_examples_common brings up the network: Ethernet (the OpenCores MAC that QEMU
# emulates) or Wi-Fi, selected in sdkconfig.defaults.
set(EXTRA_COMPONENT_DIRS $ENV{IDF_PATH}/examples/common_components/protocol_examples_common)

include($ENV{IDF_PATH}/tools/cmake/project.cmake)
project(openmhs_esp32_mqtt_node)
