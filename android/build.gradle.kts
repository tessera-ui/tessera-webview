plugins {
    id("com.android.library")
}

android {
    namespace = "com.tessera.webview"
    compileSdk = 35
    defaultConfig { minSdk = 24 }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    defaultConfig { consumerProguardFiles("consumer-rules.pro") }
}

dependencies {
    implementation("androidx.webkit:webkit:1.14.0")
}
