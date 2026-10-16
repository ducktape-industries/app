# The phone builds (wasm-mobile SPEC §11.2). `crates/phone` is built per
# phone target, never with `--workspace`, and as the library each launcher
# takes: a shared one for the Android Activity, a static one for the iOS app.
#
#   make android            the APK, x86_64 (the emulator); ANDROID_ABI=arm64-v8a for a phone
#   make android-run        installs it and starts it (adb's one device)
#   make ios-sim            the simulator app (on a Mac)
#   make ios-archive        the App Store archive, unsigned (on a Mac); BUILD=<n> is its CFBundleVersion
#
# FEATURES=ax-door (the default) is the gate build with the AX test door on
# 127.0.0.1:7777; FEATURES= builds without it.

ROOT := $(abspath $(dir $(lastword $(MAKEFILE_LIST)))/..)
export CARGO_TARGET_DIR ?= $(ROOT)/target
FEATURES ?= ax-door
CARGO_FEATURES := $(if $(FEATURES),--features $(FEATURES))

ANDROID_ABI ?= x86_64
ANDROID_TRIPLE_x86_64 := x86_64-linux-android
ANDROID_TRIPLE_arm64-v8a := aarch64-linux-android
ANDROID_OUT := $(ROOT)/phone/android/app/build/rust
APK := $(ROOT)/phone/android/app/build/outputs/apk/debug/app-debug.apk

IOS_OUT := $(ROOT)/phone/ios/build/rust
IOS_APP := $(ROOT)/phone/ios/build/derived/Build/Products/Debug-iphonesimulator/Ducktape.app
IOS_ARCHIVE := $(ROOT)/phone/ios/build/Ducktape.xcarchive
BUILD ?= 1

.PHONY: android android-run ios-sim ios-archive

android:
	cd $(ROOT) && cargo ndk -t $(ANDROID_ABI) -P 26 rustc -p ducktape-phone --lib --profile phone $(CARGO_FEATURES) --crate-type cdylib
	mkdir -p $(ANDROID_OUT)/jniLibs/$(ANDROID_ABI) $(ANDROID_OUT)/assets/fonts
	cp $(CARGO_TARGET_DIR)/$(ANDROID_TRIPLE_$(ANDROID_ABI))/phone/libducktape_phone.so $(ANDROID_OUT)/jniLibs/$(ANDROID_ABI)/
	# gm reads the colour emoji face from the APK's assets
	cp $(ROOT)/assets/fonts/NotoColorEmoji.ttf $(ANDROID_OUT)/assets/fonts/
	cd $(ROOT)/phone/android && ./gradlew --quiet assembleDebug

android-run: android
	adb install -r $(APK)
	adb shell am force-stop industries.ducktape.mobile
	adb shell am start -n industries.ducktape.mobile/.MainActivity

# The static library, the frameworks rustc says it needs (an xcconfig the
# XcodeGen project reads), the project, the app. On the shared Mac: four jobs.
ios-sim:
	mkdir -p $(IOS_OUT)/iphonesimulator
	cd $(ROOT) && IPHONEOS_DEPLOYMENT_TARGET=15.0 cargo rustc -p ducktape-phone --lib --profile phone \
		--target aarch64-apple-ios-sim $(CARGO_FEATURES) --crate-type staticlib -- --print native-static-libs \
		2> $(IOS_OUT)/rustc.log || { cat $(IOS_OUT)/rustc.log; exit 1; }
	printf 'RUST_NATIVE_LIBS = %s\n' "$$(sed -n 's/^note: native-static-libs: //p' $(IOS_OUT)/rustc.log | tail -1)" > $(IOS_OUT)/link.xcconfig
	cp $(CARGO_TARGET_DIR)/aarch64-apple-ios-sim/phone/libducktape_phone.a $(IOS_OUT)/iphonesimulator/
	cd $(ROOT)/phone/ios && xcodegen generate --quiet
	cd $(ROOT)/phone/ios && xcodebuild -project Ducktape.xcodeproj -scheme Ducktape -sdk iphonesimulator \
		-configuration Debug -destination 'generic/platform=iOS Simulator' -derivedDataPath build/derived -jobs 4 -quiet build
	@echo $(IOS_APP)

# What ships to TestFlight (.github/workflows/testflight.yml): the phone's
# library without the AX door, whatever FEATURES says, archived unsigned; the
# workflow signs it on export.
ios-archive:
	mkdir -p $(IOS_OUT)/iphoneos
	cd $(ROOT) && IPHONEOS_DEPLOYMENT_TARGET=15.0 cargo rustc -p ducktape-phone --lib --profile phone \
		--target aarch64-apple-ios --crate-type staticlib -- --print native-static-libs \
		2> $(IOS_OUT)/rustc.log || { cat $(IOS_OUT)/rustc.log; exit 1; }
	printf 'RUST_NATIVE_LIBS = %s\n' "$$(sed -n 's/^note: native-static-libs: //p' $(IOS_OUT)/rustc.log | tail -1)" > $(IOS_OUT)/link.xcconfig
	cp $(CARGO_TARGET_DIR)/aarch64-apple-ios/phone/libducktape_phone.a $(IOS_OUT)/iphoneos/
	cd $(ROOT)/phone/ios && xcodegen generate --quiet
	cd $(ROOT)/phone/ios && xcodebuild -project Ducktape.xcodeproj -scheme Ducktape -sdk iphoneos \
		-configuration Release -destination 'generic/platform=iOS' -derivedDataPath build/derived -jobs 4 -quiet \
		archive -archivePath $(IOS_ARCHIVE) CODE_SIGNING_ALLOWED=NO CURRENT_PROJECT_VERSION=$(BUILD)
	@echo $(IOS_ARCHIVE)
