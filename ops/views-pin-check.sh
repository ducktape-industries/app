# shellcheck shell=bash disable=SC2034 # views and views_rev are the caller's
# Sourced by ops/bundle-app-macos.sh and ops/stage-release.sh: a release
# ships the views ops/views.rev pins, and its views/VIEWS_REV says which.
#
# pinned_views <repo> sets `views` (the directory to ship) and `views_rev`
# (what the shipped views/VIEWS_REV holds). With DUCKTAPE_VIEWS_DIR unset the
# pin is staged (ops/stage-views.sh --release) and target/views shipped. A set
# DUCKTAPE_VIEWS_DIR is refused unless its VIEWS_REV equals the pin;
# DUCKTAPE_VIEWS_UNPINNED=1 ships it anyway, loudly, as `unpinned <its rev>`.
pinned_views() {
  local repo=$1 pin staged
  pin=$(<"$repo/ops/views.rev")
  if [[ -z "${DUCKTAPE_VIEWS_DIR:-}" ]]; then
    "$repo/ops/stage-views.sh" --release
    views=$repo/target/views
    views_rev=$pin
  else
    views=$DUCKTAPE_VIEWS_DIR
    staged=$(cat "$views/VIEWS_REV" 2>/dev/null) || staged=""
    if [[ "$staged" == "$pin" ]]; then
      views_rev=$pin
    elif [[ "${DUCKTAPE_VIEWS_UNPINNED:-}" == 1 ]]; then
      echo "WARNING: DUCKTAPE_VIEWS_UNPINNED=1 ships the views in $views (${staged:-no VIEWS_REV}), NOT the pinned $pin; this artifact is not a release of ops/views.rev" >&2
      views_rev="unpinned${staged:+ $staged}"
    else
      echo "DUCKTAPE_VIEWS_DIR=$views holds views ${staged:-with no VIEWS_REV}, not the pinned $pin (ops/views.rev): unset it to stage the pin, or set DUCKTAPE_VIEWS_UNPINNED=1 to ship an unpinned set" >&2
      exit 1
    fi
  fi
  compgen -G "$views/*_view.wasm" >/dev/null || { echo "no *_view.wasm in $views" >&2; exit 1; }
}
