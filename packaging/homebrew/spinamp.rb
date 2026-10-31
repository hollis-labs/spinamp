# Homebrew cask for Spinamp. Lives in a tap repo, hollis-labs/homebrew-tap, at Casks/spinamp.rb.
#   brew install --cask hollis-labs/tap/spinamp
# Refresh version + checksum for a release with:  packaging/update-cask.sh 0.1.0
cask "spinamp" do
  version "0.1.0"
  sha256 "d7ee51729c7866108f6653708146be35208354bef77e503067d24e578ddb3334"

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
