# Keep the JNI bridge: its methods are resolved by name from native code, so
# R8 must not rename or strip them.
-keepclasseswithmembernames class com.iosemu.emulator.emu.EmulatorBridge {
    native <methods>;
}
-keep class com.iosemu.emulator.emu.EmulatorBridge { *; }
