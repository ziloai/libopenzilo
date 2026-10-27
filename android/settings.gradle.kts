pluginManagement {
    resolutionStrategy {
        eachPlugin {
            if (requested.id.id.startsWith("com.android."))
                useModule("com.android.tools.build:gradle:${requested.version}")
        }
    }
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositories {
        maven { url = uri("../dist/maven") }
        google()
        mavenCentral()
    }
}

rootProject.name = "libopenzilo"

include(":sdk", ":sample")
