plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "com.iosemu.emulator"
    compileSdk = 34

    defaultConfig {
        applicationId = "com.iosemu.emulator"
        minSdk = 24        // Android 7.0: first release with reliable 64-bit-only support
        targetSdk = 34
        versionCode = 1
        versionName = "0.1.0"

        // 64-bit only — the core executes AArch64 guest code natively.
        ndk {
            abiFilters += "arm64-v8a"
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }

    buildFeatures {
        compose = true
    }
    composeOptions {
        kotlinCompilerExtensionVersion = "1.5.14"
    }

    // The .so files produced by cargo-ndk land here and are packaged into the APK.
    sourceSets["main"].jniLibs.srcDir(layout.buildDirectory.dir("rustJniLibs"))
}

// ---------------------------------------------------------------------------
// cargo-ndk: cross-compile the Rust `ios-emu-jni` crate (which statically links
// the native C backend via its build.rs) into `libios_emu_jni.so` for arm64-v8a.
//
// Prerequisites on the build machine:
//   * Android NDK (ANDROID_NDK_HOME / ndkVersion)
//   * `rustup target add aarch64-linux-android`
//   * `cargo install cargo-ndk`
//
// cargo-ndk writes `<out>/arm64-v8a/libios_emu_jni.so`, exactly the layout the
// jniLibs source set expects.
// ---------------------------------------------------------------------------
val workspaceRoot: File = rootProject.projectDir.parentFile // repo root (holds Cargo.toml)
val rustJniLibs = layout.buildDirectory.dir("rustJniLibs")

val cargoNdkBuild by tasks.registering(Exec::class) {
    group = "rust"
    description = "Build the Rust cdylib for arm64-v8a with cargo-ndk"
    workingDir = workspaceRoot
    outputs.dir(rustJniLibs)

    commandLine(
        "cargo", "ndk",
        "-t", "arm64-v8a",
        "-o", rustJniLibs.get().asFile.absolutePath,
        "build", "--release",
        "-p", "ios-emu-jni",
        "--features", "android",
    )
}

// Ensure the native library exists before the APK is assembled.
tasks.named("preBuild").configure {
    dependsOn(cargoNdkBuild)
}

dependencies {
    implementation("androidx.core:core-ktx:1.13.1")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.8.4")
    implementation("androidx.activity:activity-compose:1.9.1")

    val composeBom = platform("androidx.compose:compose-bom:2024.06.00")
    implementation(composeBom)
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-graphics")
    implementation("androidx.compose.material3:material3")

    debugImplementation("androidx.compose.ui:ui-tooling")
}
