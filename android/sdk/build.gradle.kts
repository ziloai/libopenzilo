plugins {
    id("com.android.library")
    id("org.jetbrains.kotlin.android")
    id("maven-publish")
}

android {
    namespace = "com.zilo.sdk"
    compileSdk = 36
    buildToolsVersion = "36.0.0"
    ndkVersion = "28.2.13676358"
    defaultConfig {
        minSdk = 26
        consumerProguardFiles("consumer-rules.pro")
        ndk { abiFilters += "arm64-v8a" }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    sourceSets["main"].java.srcDir("build/generated/uniffi")
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
                version = "0.1.0"
            }
        }
        repositories { maven { url = uri("../../dist/maven") } }
    }
}

val buildNative by
    tasks.registering(Exec::class) {
        workingDir = projectDir.resolve("../..").canonicalFile
        commandLine("python3", "tools/build_android.py")
    }

tasks.named("preBuild") { dependsOn(buildNative) }

tasks.withType<Test>().configureEach {
    val cargoTarget =
        System.getenv("CARGO_TARGET_DIR") ?: projectDir.resolve("../../target").canonicalPath
    systemProperty("jna.library.path", "$cargoTarget/debug")
    val nativeSuffix = if (System.getProperty("os.name").startsWith("Mac")) "dylib" else "so"
    inputs.file("$cargoTarget/debug/libopenzilo_core.$nativeSuffix")
}
