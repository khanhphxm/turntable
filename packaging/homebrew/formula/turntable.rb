# Copy to Formula/turntable.rb in khanhphxm/homebrew-tap and substitute the tokens.
class Turntable < Formula
  desc "Lightweight native Spotify player"
  homepage "https://github.com/khanhphxm/turntable"
  version "@VERSION@"
  license "MIT"

  on_linux do
    on_intel do
      url "https://github.com/khanhphxm/turntable/releases/download/v#{version}/turntable-#{version}-linux-x86_64.tar.gz"
      sha256 "@LINUX_X86_64_SHA256@"
    end
    on_arm do
      url "https://github.com/khanhphxm/turntable/releases/download/v#{version}/turntable-#{version}-linux-aarch64.tar.gz"
      sha256 "@LINUX_AARCH64_SHA256@"
    end
  end

  depends_on :linux
  depends_on "alsa-lib"
  depends_on "dbus"
  depends_on "fontconfig"
  depends_on "freetype"
  depends_on "libx11"
  depends_on "libxcb"
  depends_on "libxcursor"
  depends_on "libxfixes"
  depends_on "libxi"
  depends_on "libxinerama"
  depends_on "libxkbcommon"
  depends_on "libxrandr"
  depends_on "libxrender"
  depends_on "mesa"
  depends_on "openssl@3"
  depends_on "pipewire"
  depends_on "vulkan-loader"
  depends_on "wayland"

  def install
    bin.install "bin/turntable"
    share.install "share/applications", "share/icons"
    pkgshare.install "INSTALL.md", "LICENSE"
  end

  def caveats
    <<~EOS
      Requires glibc 2.39+, a Wayland/X11 graphical session, and a working Vulkan
      GPU driver (install it with your distribution's package manager).
      This is a desktop app; run turntable from your graphical session.
      To expose it in your desktop menu, copy #{share}/applications/turntable.desktop
      and #{share}/icons/hicolor into ~/.local/share/applications and
      ~/.local/share/icons respectively. Ensure #{HOMEBREW_PREFIX}/bin is on PATH.
    EOS
  end

  test do
    assert_path_exists bin/"turntable"
    assert_path_exists share/"applications/turntable.desktop"
    assert_path_exists share/"icons/hicolor/256x256/apps/turntable.png"
  end
end
