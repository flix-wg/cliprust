cask "cliprust" do
  version "0.1.1"
  sha256 "ecf6e705919484d70abe9d24465bc2caa5c919acf2bb989c5d5c6d34ece1de52"

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
