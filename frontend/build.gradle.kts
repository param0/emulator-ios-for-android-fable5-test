// Top-level build file. Plugin versions are declared here and applied per-module.
plugins {
    id("com.android.application") version "8.5.2" apply false
    id("org.jetbrains.kotlin.android") version "1.9.24" apply false
    // Builds the Rust `ios-emu-jni` cdylib for each Android ABI and drops the
    // resulting `.so` into the app's jniLibs.
    id("org.mozilla.rust-android-gradle.rust-android") version "0.9.4" apply false
}
