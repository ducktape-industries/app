#!/bin/sh
# The privacy gate before an archive ships (wasm-mobile SPEC §14): every
# required-reason API category the app's binary reaches must be declared in
# the PrivacyInfo.xcprivacy the app carries, else this fails. App Store
# Connect's ITMS-91053 mail is the second check, not the first.
#
#   phone/ios/privacy-scan.sh <Ducktape.app>
#
# What the binary reaches: its undefined symbols (`nm -u`: C functions, the
# Foundation keys, Objective-C classes) and the Objective-C selectors it
# sends. The categories and their APIs are Apple's list ("Describing use of
# required reason API"); a system framework's own use is its own manifest's.
set -eu

app=${1:?usage: privacy-scan.sh <Ducktape.app>}
exe=$app/$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$app/Info.plist")
declared=$(plutil -convert json -o - "$app/PrivacyInfo.xcprivacy")
symbols=$(nm -u "$exe")
selectors=$(strings -a "$exe")

failed=0
while read -r category apis; do
  seen=
  for api in $apis; do
    case $api in
      sel:*) printf '%s\n' "$selectors" | grep -qxF "${api#sel:}" && seen="$seen ${api#sel:}" ;;
      *) printf '%s\n' "$symbols" | grep -qxF "$api" && seen="$seen $api" ;;
    esac
  done
  case $declared in
    *"NSPrivacyAccessedAPICategory$category\""*) listed=yes ;;
    *) listed=no ;;
  esac
  if [ -n "$seen" ] && [ $listed = no ]; then
    echo "::error::$category is reached ($seen ) and PrivacyInfo.xcprivacy does not declare it"
    failed=1
  elif [ -n "$seen" ]; then
    echo "$category: declared, reached by$seen"
  elif [ $listed = yes ]; then
    echo "$category: declared, not reached by the binary"
  fi
done <<'EOF'
FileTimestamp _stat _fstat _fstatat _lstat _getattrlist _fgetattrlist _getattrlistat _getattrlistbulk _NSFileCreationDate _NSFileModificationDate _NSURLContentModificationDateKey _NSURLCreationDateKey sel:fileModificationDate
SystemBootTime _mach_absolute_time sel:systemUptime
DiskSpace _statfs _statvfs _fstatfs _fstatvfs _getattrlist _fgetattrlist _getattrlistat _NSFileSystemFreeSize _NSFileSystemSize _NSURLVolumeAvailableCapacityKey _NSURLVolumeAvailableCapacityForImportantUsageKey _NSURLVolumeAvailableCapacityForOpportunisticUsageKey _NSURLVolumeTotalCapacityKey
ActiveKeyboards sel:activeInputModes
UserDefaults _OBJC_CLASS_$_NSUserDefaults
EOF
exit $failed
