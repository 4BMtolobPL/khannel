# Homebrew Cask 템플릿 — publish.yml이 릴리스 게시 때 버전과 dmg sha256을 채워
# 4BMtolobPL/homebrew-tap의 Casks/khannel.rb로 올린다. 맨 앞의 `#` 주석 줄은 올릴 때 지운다.
cask "khannel" do
  version "@VERSION@"
  sha256 "@SHA256@"

  url "https://github.com/4BMtolobPL/khannel/releases/download/v#{version}/khannel-#{version}-macos-arm64.dmg"
  name "khannel"
  desc "IRC client built with GPUI"
  homepage "https://github.com/4BMtolobPL/khannel"

  livecheck do
    url :url
    strategy :github_latest
  end

  depends_on arch: :arm64
  depends_on :macos

  app "khannel.app"

  zap trash: [
    "~/Library/Application Support/com.kimtalmo.khannel",
    "~/Library/Saved Application State/com.kimtalmo.khannel.savedState",
  ]

  caveats <<~EOS
    khannel is not notarized by Apple (ad-hoc signed), so macOS blocks the first launch.
    Open it once, then allow it in System Settings > Privacy & Security > "Open Anyway".
  EOS
end
