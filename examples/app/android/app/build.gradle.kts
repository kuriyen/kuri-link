plugins {
    id("com.android.application")
    // The Flutter Gradle Plugin must be applied after the Android and Kotlin Gradle plugins.
    id("dev.flutter.flutter-gradle-plugin")
}

// The app's one native library, libkuri_link_echo.so: the Kuri Link bindings
// and the echo back end, built by Cargo from examples/echo, as
// linux/CMakeLists.txt does for Linux. cargo-ndk points Cargo at the NDK's
// linker for each ABI; install it with `cargo install cargo-ndk`, and the
// targets with `rustup target add aarch64-linux-android x86_64-linux-android`.
// The ABIs are phones' and the emulator's, by Rust target.
val abis = mapOf("arm64-v8a" to "aarch64-linux-android", "x86_64" to "x86_64-linux-android")
val repositoryRoot = file("../../../..")

fun rustLibs(buildType: String) = layout.buildDirectory.dir("rustJniLibs/$buildType").get().asFile

android {
    namespace = "io.github.kuriyen.kuri_link_example"
    compileSdk = flutter.compileSdkVersion
    ndkVersion = flutter.ndkVersion

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    defaultConfig {
        applicationId = "io.github.kuriyen.kuri_link_example"
        // You can update the following values to match your application needs.
        // For more information, see: https://flutter.dev/to/review-gradle-config.
        minSdk = flutter.minSdkVersion
        targetSdk = flutter.targetSdkVersion
        // Uses the version code from pubspec.yaml. When using split APKs, 1000 * ABI_VERSION
        // is added automatically by Flutter. (https://developer.android.com/studio/build/configure-apk-splits#configure-APK-versions)
        // You can force using the value of versionCode by specifying the `-P force-version-code-ignoring-abi=true`
        // flag during build.
        versionCode = flutter.versionCode
        versionName = flutter.versionName

        ndk {
            abiFilters += abis.keys
        }
    }

    // Flutter adds 32-bit ARM for its own engine. An APK must not offer an
    // ABI it has no library of ours for: a phone would install it, then fail
    // to load the library. Without it, such a phone refuses the install.
    packaging {
        jniLibs {
            excludes += listOf("lib/armeabi-v7a/**", "lib/x86/**")
        }
    }

    sourceSets {
        for (buildType in listOf("debug", "profile", "release")) {
            getByName(buildType).jniLibs.srcDir(rustLibs(buildType))
        }
    }

    buildTypes {
        release {
            // An example: signed with the debug key, so `flutter run --release` works.
            signingConfig = signingConfigs.getByName("debug")
        }
    }
}

kotlin {
    compilerOptions {
        jvmTarget = org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17
    }
}

flutter {
    source = "../.."
}

for (buildType in listOf("debug", "profile", "release")) {
    val name = buildType.replaceFirstChar { it.uppercase() }
    val profile = if (buildType == "debug") "debug" else "release"
    val cargo =
        tasks.register<Exec>("cargoBuild$name") {
            description = "Builds libkuri_link_echo.so with Cargo for ${abis.keys}."
            workingDir = repositoryRoot
            environment("ANDROID_NDK_HOME", android.ndkDirectory.path)
            commandLine(
                listOf("cargo", "ndk") +
                    abis.keys.flatMap { listOf("-t", it) } +
                    listOf("--platform", flutter.minSdkVersion.toString()) +
                    listOf("build", "--locked", "-p", "kuri-link-echo", "--lib") +
                    (if (profile == "release") listOf("--release") else emptyList()),
            )
            // Only this library: the dependencies' own cdylibs are not the app's.
            doLast {
                for ((abi, target) in abis) {
                    copy {
                        from(repositoryRoot.resolve("target/$target/$profile/libkuri_link_echo.so"))
                        into(rustLibs(buildType).resolve(abi))
                    }
                }
            }
        }
    tasks.matching { it.name == "merge${name}JniLibFolders" }.configureEach { dependsOn(cargo) }
}
