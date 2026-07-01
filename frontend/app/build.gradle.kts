plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.mozilla.rust-android-gradle.rust-android")
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

        // 64-bit only — the emulator core executes AArch64 guest code natively.
        ndk {
            abiFilters += listOf("arm64-v8a")
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
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

    // The native C backend (jit_exec.c / vm_bridge.c) is compiled and statically
    // linked into the Rust cdylib by the crate's build.rs, so no separate
    // externalNativeBuild block is required here.
}

// Configure the Rust build: produce `libios_emu_jni.so` for arm64 with the
// `android` feature (JNI + logcat) enabled, in release mode.
cargo {
    module = "../.."                 // path to the Cargo workspace root
    libname = "ios_emu_jni"
    targets = listOf("arm64")
    profile = "release"
    features {
        defaultAnd(arrayOf("android"))
    }
    prebuiltToolchains = true
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
    implementation("androidx.compose.material:material-icons-core")

    debugImplementation("androidx.compose.ui:ui-tooling")
}

// Ensure the Rust/native library is built before the APK is assembled.
tasks.matching { it.name.matches(Regex("merge.*JniLibFolders")) }.configureEach {
    dependsOn(tasks.named("cargoBuild"))
}
tasks.whenTaskAdded {
    if (name == "javaPreCompileDebug" || name == "javaPreCompileRelease") {
        dependsOn("cargoBuild")
    }
}
