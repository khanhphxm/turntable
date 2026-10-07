# Copy to Casks/turntable.rb in khanhphxm/homebrew-tap and substitute the tokens.
cask "turntable" do
  version "@VERSION@"
  sha256 "@MACOS_ARM64_SHA256@"

  url "https://github.com/khanhphxm/turntable/releases/download/v#{version}/turntable-#{version}-macos-arm64.zip"
  name "turntable"
  desc "Lightweight native Spotify player"
  homepage "https://github.com/khanhphxm/turntable"

  depends_on arch: :arm64
  depends_on macos: ">= :monterey"

  app "turntable.app"
end
