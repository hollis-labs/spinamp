# Homebrew cask for Spinamp. Lives in a tap repo, hollis-labs/homebrew-tap, at Casks/spinamp.rb.
#   brew install --cask hollis-labs/tap/spinamp
# Refresh version + checksum for a release with:  packaging/update-cask.sh 0.1.0
cask "spinamp" do
  version "0.2.0"
  sha256 "b581e5d2fb602b74acbf8102c869417fe75ad4c3cd937367fee04f2b1d3217bb"

  url "https://github.com/hollis-labs/spinamp/releases/download/v#{version}/Spinamp_#{version}_aarch64.dmg"
  name "Spinamp"
  desc "Playlist-first music player that learns what you love"
  homepage "https://spinamp.fm"

  # Apple Silicon only for now (the Intel build is not published yet). Signed with a Developer ID
  # certificate and notarized by Apple, so no quarantine workaround is needed.
  depends_on arch: :arm64
  depends_on macos: :ventura

  app "Spinamp.app"

  zap trash: [
    "~/Library/Application Support/Spinamp",
    "~/Library/Caches/fm.spinamp.player",
    "~/Library/WebKit/fm.spinamp.player",
  ]
end
