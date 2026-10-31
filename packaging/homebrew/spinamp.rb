# Homebrew cask for Spinamp. Lives in a tap repo, hollis-labs/homebrew-tap, at Casks/spinamp.rb.
#   brew install --cask hollis-labs/tap/spinamp
# Refresh version + checksum for a release with:  packaging/update-cask.sh 0.1.0
cask "spinamp" do
  version "0.1.0"
  sha256 "52740db9f94ef5656a931e3eee1d057da0cfb5781e4e9663a2499890b2c6729b"

  url "https://github.com/hollis-labs/spinamp/releases/download/v#{version}/Spinamp_#{version}_aarch64.dmg"
  name "Spinamp"
  desc "Playlist-first music player that learns what you love"
  homepage "https://spinamp.fm"

  # Apple Silicon only for now (the Intel build is not published yet).
  depends_on arch: :arm64
  depends_on macos: :ventura

  app "Spinamp.app"

  # Early releases are not notarized yet, so macOS would block the first launch of a downloaded app.
  # Remove this block once releases are signed + notarized (see RELEASING.md).
  postflight_steps do
    run "/usr/bin/xattr", args: ["-dr", "com.apple.quarantine", "{{appdir}}/Spinamp.app"], must_succeed: false
  end

  zap trash: [
    "~/Library/Application Support/Spinamp",
    "~/Library/Caches/fm.spinamp.player",
    "~/Library/WebKit/fm.spinamp.player",
  ]
end
