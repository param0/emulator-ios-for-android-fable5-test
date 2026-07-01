package com.iosemu.emulator.emu

/**
 * JNI facade over the Rust core (`libios_emu_jni.so`, built by cargo-ndk).
 *
 * The native library is produced from the `ios-emu-jni` crate and statically
 * absorbs the C AArch64 backend. Each method is a thin marshaller:
 *  - [nativeScan] returns a JSON array of app descriptors,
 *  - [nativeIcon] returns raw PNG bytes for one IPA (empty if none),
 *  - [nativeLaunch] boots an app and blocks until it exits, returning the exit
 *    code (negative values indicate a host-side failure at/ before load).
 */
object EmulatorBridge {

    init {
        System.loadLibrary("ios_emu_jni")
    }

    /** Scan [directory] for `.ipa` files; returns a JSON array string. */
    external fun nativeScan(directory: String): String

    /** Fetch the primary icon PNG for the IPA at [ipaPath]; empty if none. */
    external fun nativeIcon(ipaPath: String): ByteArray

    /**
     * Launch the app at [ipaPath], creating its sandbox under [containersRoot]
     * and resolving system frameworks against [systemRoot]. Blocks until exit.
     */
    external fun nativeLaunch(
        ipaPath: String,
        containersRoot: String,
        systemRoot: String,
    ): Int
}
