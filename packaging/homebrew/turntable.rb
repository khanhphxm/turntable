# Rendered by render.py into Casks/turntable.rb in khanhphxm/homebrew-tap.
cask "turntable" do
  version "@VERSION@"
  sha256 "@MACOS_UNIVERSAL_SHA256@"

  url "https://github.com/khanhphxm/turntable/releases/download/v#{version}/turntable-#{version}-macos-universal.zip"
  name "Turntable"
  desc "Lightweight native Spotify player"
  homepage "https://github.com/khanhphxm/turntable"

  depends_on macos: :monterey

  app "Turntable.app"
end
