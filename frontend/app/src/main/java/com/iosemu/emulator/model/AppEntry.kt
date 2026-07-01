package com.iosemu.emulator.model

/**
 * Display model for one discovered iOS application, parsed from the JSON the
 * native scanner returns.
 */
data class AppEntry(
    val name: String,
    val bundleId: String,
    val minOs: String,
    val ipaPath: String,
    val hasIcon: Boolean,
)
