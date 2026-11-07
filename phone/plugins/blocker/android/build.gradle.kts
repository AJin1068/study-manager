// Library module of the blocker plugin. Compiled and unit-tested on Windows (docs/stage-5-verification.md
// V5-4) via `phone/scripts/wire-blocker.mjs`, which includes it in the generated Tauri Android project;
// that project provides `:tauri-android`. APK packaging is still unverified (V5-1).
plugins {
    id("com.android.library")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "app.studymanager.blocker"
    compileSdk = 34

    defaultConfig {
        minSdk = 26
        consumerProguardFiles("consumer-rules.pro")
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }
    sourceSets {
        getByName("test") {
            resources.srcDirs("../../../../fixtures/phone")
        }
    }
}

dependencies {
    implementation("androidx.core:core-ktx:1.12.0")
    implementation(project(":tauri-android"))
    testImplementation("junit:junit:4.13.2")
    testImplementation("org.json:json:20231013")
}
