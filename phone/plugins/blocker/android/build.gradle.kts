// Library module of the blocker plugin. Source only: never built in this repository's environment
// (no Android SDK) - see docs/stage-5-verification.md V5-4. The Tauri Android project of the phone
// app includes this module and provides `:tauri-android`.
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
