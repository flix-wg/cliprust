cask "cliprust" do
  version "0.1.0"
  sha256 "38d80241f6cdf64ccbd3dada8ee9b2b00fd94a1a6f12150f0503e23db4496049"

  url "https://github.com/flix-wg/cliprust/releases/download/v#{version}/ClipRust-#{version}-macos-arm64.zip"
  name "ClipRust"
  desc "Fast, low-memory clipboard history"
  homepage "https://github.com/flix-wg/cliprust"

  livecheck do
    url :url
    strategy :github_latest
  end

  depends_on arch: :arm64
  depends_on macos: :tahoe

  app "ClipRust.app"

  zap trash: [
    "~/Library/Application Support/cliprust",
    "~/Library/Preferences/app.cliprust.ClipRust.plist",
  ]
end
