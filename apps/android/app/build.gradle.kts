import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "com.remoteview.client"
    compileSdk = 36

    defaultConfig {
        applicationId = "com.remoteview.client"
        minSdk = 26
        targetSdk = 36
        // CI đặt RV_VERSION_CODE = số lần chạy, để bản mới cài đè được bản cũ.
        versionCode = (System.getenv("RV_VERSION_CODE") ?: "1").toInt()
        versionName = "0.1." + (System.getenv("RV_VERSION_CODE") ?: "0")

        // Mã commit của bản build (CI: RV_BUILD_SHA nếu có, nếu không thì GITHUB_SHA của GitHub Actions); build cục bộ = "dev".
        val gitSha = (System.getenv("RV_BUILD_SHA") ?: System.getenv("GITHUB_SHA") ?: "dev")
            .trim().ifEmpty { "dev" }.replace(Regex("[^0-9A-Za-z_-]"), "")
        buildConfigField("String", "GIT_SHA", "\"$gitSha\"")

        // Chỉ arm64: S26+ và Tab S10+ đều là arm64-v8a.
        ndk { abiFilters += listOf("arm64-v8a") }
    }

    signingConfigs {
        // Khóa cố định nằm trong repo: mọi bản build (máy nào, CI lần nào) cùng một chữ ký,
        // nên APK mới cài đè được APK cũ mà không phải gỡ. Không dùng cho phát hành Play Store.
        getByName("debug") {
            storeFile = file("debug.keystore")
            storePassword = "android"
            keyAlias = "androiddebugkey"
            keyPassword = "android"
        }
        // Khóa phát hành riêng: chỉ dùng khi CI có secrets RV_KEYSTORE_* (không lưu trong repo).
        create("release") {
            val ks = System.getenv("RV_KEYSTORE_PATH")
            if (ks != null) {
                storeFile = file(ks)
                storePassword = System.getenv("RV_KEYSTORE_PASSWORD")
                keyAlias = System.getenv("RV_KEY_ALIAS")
                keyPassword = System.getenv("RV_KEY_PASSWORD")
            } else {
                initWith(getByName("debug"))
            }
        }
    }

    buildTypes {
        debug {
            signingConfig = signingConfigs.getByName("debug")
        }
        release {
            signingConfig = signingConfigs.getByName("release")
            isMinifyEnabled = true
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

    buildFeatures {
        compose = true
        buildConfig = true
    }

    // librv_core.so (Rust, crates/android-core) do cargo-ndk đặt vào src/main/jniLibs/arm64-v8a/ (thư mục jniLibs mặc định,
    // tự được đóng gói vào APK). Thư mục này bị .gitignore.
}

kotlin {
    compilerOptions { jvmTarget.set(JvmTarget.JVM_17) }
}

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2025.05.01")
    implementation(composeBom)
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material3:material3-window-size-class")
    implementation("androidx.compose.material:material-icons-extended")
    implementation("androidx.compose.foundation:foundation")
    debugImplementation("androidx.compose.ui:ui-tooling")

    implementation("androidx.core:core-ktx:1.16.0")
    implementation("androidx.activity:activity-compose:1.10.1")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.9.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.10.2")
}
