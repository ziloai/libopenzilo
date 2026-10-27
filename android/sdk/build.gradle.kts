plugins {
    id("com.android.library")
    id("org.jetbrains.kotlin.android")
    id("maven-publish")
}

val sdkAbis = providers.gradleProperty("openziloAbis").orElse("arm64-v8a").get().split(",")
val coreVersion = Regex("(?m)^version = \"([^\"]+)\"$").find(projectDir.resolve("../../Cargo.toml").readText())!!.groupValues[1]

android {
    namespace = "com.zilo.sdk"
    compileSdk = 36
    buildToolsVersion = "36.0.0"
    ndkVersion = "28.2.13676358"
    defaultConfig {
        minSdk = 26
        consumerProguardFiles("consumer-rules.pro")
        ndk { abiFilters += sdkAbis }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    sourceSets["main"].java.srcDir("build/generated/uniffi")
    sourceSets["main"].jniLibs.setSrcDirs(listOf("build/generated/jniLibs"))
    sourceSets["test"].resources.srcDir("../../tests/fixtures")
    publishing { singleVariant("release") { withSourcesJar() } }
}

kotlin {
    jvmToolchain(21)
    compilerOptions { jvmTarget = org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17 }
}

dependencies {
    api("androidx.annotation:annotation:1.9.1")
    api("net.java.dev.jna:jna:5.17.0@aar")
    api("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.10.2")
    testImplementation("junit:junit:4.13.2")
    testRuntimeOnly("net.java.dev.jna:jna:5.17.0@jar")
}

afterEvaluate {
    publishing {
        publications {
            create<MavenPublication>("release") {
                from(components["release"])
                groupId = "com.zilo"
                artifactId = "openzilo-sdk"
                version = coreVersion
                pom {
                    name.set("OpenZilo SDK")
                    description.set("OpenZilo shared protocol and Android BLE SDK")
                    url.set("https://github.com/ziloai/libopenzilo")
                    licenses { license { name.set("Mozilla Public License 2.0"); url.set("https://www.mozilla.org/MPL/2.0/") } }
                }
            }
        }
        repositories { maven { url = uri("../../dist/maven") } }
    }
}

val buildNative by
    tasks.registering(Exec::class) {
        workingDir = projectDir.resolve("../..").canonicalFile
        commandLine(listOf("python3", "tools/build_android.py") + sdkAbis.flatMap { listOf("--abi", it) })
    }

tasks.named("preBuild") { dependsOn(buildNative) }

tasks.withType<Test>().configureEach {
    val cargoTarget =
        System.getenv("CARGO_TARGET_DIR") ?: projectDir.resolve("../../target").canonicalPath
    systemProperty("jna.library.path", "$cargoTarget/debug")
    val nativeSuffix = if (System.getProperty("os.name").startsWith("Mac")) "dylib" else "so"
    inputs.file("$cargoTarget/debug/liblibopenzilo.$nativeSuffix")
}
