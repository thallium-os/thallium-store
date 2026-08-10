cmake_minimum_required(VERSION 3.21)

project(ThalliumStoreNative VERSION 0.1.11 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_AUTOMOC ON)

find_package(Qt6 6.4 REQUIRED COMPONENTS
    Core
    Gui
    Network
    Qml
    Quick
    QuickControls2
)

qt_standard_project_setup(REQUIRES 6.4)

qt_add_executable(thallium-store-native
    src/main.cpp
    src/storeclient.cpp
    src/storeclient.h
)

set(THALLIUM_QML "${CMAKE_CURRENT_SOURCE_DIR}/../ui/shell.qml")
set_source_files_properties("${THALLIUM_QML}" PROPERTIES QT_RESOURCE_ALIAS Main.qml)

qt_add_qml_module(thallium-store-native
    URI Thallium.Store
    VERSION 1.0
    QML_FILES "${THALLIUM_QML}"
    RESOURCE_PREFIX /qt/qml
)

target_link_libraries(thallium-store-native PRIVATE
    Qt6::Core
    Qt6::Gui
    Qt6::Network
    Qt6::Qml
    Qt6::Quick
    Qt6::QuickControls2
)

set_target_properties(thallium-store-native PROPERTIES
    OUTPUT_NAME thallium-store-native
    WIN32_EXECUTABLE TRUE
)

include(GNUInstallDirs)
install(TARGETS thallium-store-native
    BUNDLE DESTINATION .
    RUNTIME DESTINATION "${CMAKE_INSTALL_BINDIR}"
)
