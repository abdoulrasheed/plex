class Plex < Formula
  desc "Local-first code intelligence engine with semantic search and call graphs"
  homepage "https://github.com/abdoulrasheed/plex"
  version "0.1.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/abdoulrasheed/plex/releases/download/v#{version}/plex-darwin-arm64.tar.gz"
      sha256 "PLACEHOLDER"
    else
      url "https://github.com/abdoulrasheed/plex/releases/download/v#{version}/plex-darwin-x64.tar.gz"
      sha256 "PLACEHOLDER"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/abdoulrasheed/plex/releases/download/v#{version}/plex-linux-arm64.tar.gz"
      sha256 "PLACEHOLDER"
    else
      url "https://github.com/abdoulrasheed/plex/releases/download/v#{version}/plex-linux-x64.tar.gz"
      sha256 "PLACEHOLDER"
    end
  end

  def install
    bin.install "plex"
  end

  test do
    assert_match "plex", shell_output("#{bin}/plex --version")
  end
end
