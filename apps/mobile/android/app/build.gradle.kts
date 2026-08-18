plugins {
    id("com.android.application")
    id("kotlin-android")
    // The Flutter Gradle Plugin must be applied after the Android and Kotlin Gradle plugins.
    id("dev.flutter.flutter-gradle-plugin")
}

android {
    namespace = "dev.anvil.anvil"
    compileSdk = flutter.compileSdkVersion
    ndkVersion = flutter.ndkVersion

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = JavaVersion.VERSION_17.toString()
    }

    defaultConfig {
        // TODO: Specify your own unique Application ID (https://developer.android.com/studio/build/application-id.html).
        applicationId = "dev.anvil.anvil"
        // You can update the following values to match your application needs.
        // For more information, see: https://flutter.dev/to/review-gradle-config.
        minSdk = flutter.minSdkVersion
        targetSdk = flutter.targetSdkVersion
        versionCode = flutter.versionCode
        versionName = flutter.versionName
    }

    buildTypes {
        release {
            // TODO: Add your own signing config for the release build.
            // Signing with the debug keys for now, so `flutter run --release` works.
            signingConfig = signingConfigs.getByName("debug")
        }
    }
}

flutter {
    source = "../.."
}

val buildAnvilRust by tasks.registering(Exec::class) {
    val repositoryRoot = rootProject.projectDir.resolve("../../..").canonicalFile
    val jniLibsDirectory = project.projectDir.resolve("src/main/jniLibs")

    workingDir(repositoryRoot)
    commandLine(
        "/bin/sh",
        rootProject.file("build_rust.sh").absolutePath,
        repositoryRoot.absolutePath,
        jniLibsDirectory.absolutePath,
    )
}

tasks.named("preBuild") {
    dependsOn(buildAnvilRust)
}
