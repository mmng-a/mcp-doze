# mcp-doze

[English](README.md) | [日本語](README.ja.md)

**使っていないMCPサーバーのリソース消費を減らしませんか？**

Codexでは、[サブエージェントも親のMCP設定を引き継ぐ](https://learn.chatgpt.com/docs/agent-configuration/subagents)。チャットやサブエージェントが増えると、同じローカルMCPサーバーが何組も起動することがある。

1つあたりは数十MBでも、起動数が増えると合計はGB単位に達する。ツール一覧の取得だけでもサーバーが起動するため、使わないツールの分もメモリを消費する。

mcp-dozeは、初期化や一覧取得にはキャッシュから応答し、操作が届いた段階で実サーバーを起動するラッパー。MCPを登録したまま、未使用サーバーの常駐を減らせる。Codexプラグインに同梱されたローカルMCPにも対応。

## インストール

検証済みの環境はmacOS。実装はUnix向けで、Windowsには未対応。

Homebrew 7以降では、tapを追加し、mcp-dozeのFormulaを信頼対象に登録してからインストールする。

```sh
brew tap mmng-a/tap
brew trust --formula mmng-a/tap/mcp-doze
brew install mmng-a/tap/mcp-doze
```

## 使い方

既存のMCP登録で、起動コマンドをmcp-doze経由に変更する。Codexの`config.toml`でNode.js製のサーバーを使う場合の例：

```toml
[mcp_servers.example]
command = "/absolute/path/to/mcp-doze"
args = ["run", "--auto-refresh", "--", "node", "/absolute/path/to/server.mjs"]
```

mcp-dozeのパスは`command -v mcp-doze`で確認できる。元の環境変数・作業ディレクトリ・承認方針・ツール許可リストはそのまま維持する。

## Codexプラグイン

同梱MCPにも適用する場合は、Codexの起動前に次を実行する。

```sh
mcp-doze codex sync-plugins
```

元のプラグイン内の起動定義を変更し、識別情報・認証方針・スキル・フック定義を維持する。プラグイン更新で設定が置き換わる場合があるため、起動前に再実行する。[詳しい適用方法](docs/usage.md#codex-plugins)

## 動作と対応範囲

- 対象はローカルstdio接続のMCP。HTTP接続は対象外。
- キャッシュがない場合や更新が必要な場合は、一覧取得のため一時的に実サーバーを起動する。
- dozeはMCP接続ごとに1プロセス起動する。使用を開始した実サーバーは、クライアントが接続を閉じるまで状態を保持し、待機時間による自動停止は行わない。

## ドキュメント

以下は英語の詳細資料。

- [設定と動作](docs/usage.md)：オプション・キャッシュ・認証・プラグイン対応
- [設計](docs/design.md)・[検証結果とメモリ測定](docs/validation.md)
- [ソースからのビルドと開発](docs/development.md)・[Homebrew配布手順](docs/homebrew.md)

## ライセンス

[MIT](LICENSE)。設計は[lazy-mcp-wrapper](https://github.com/sleticalboy/lazy-mcp-wrapper)を参考にした。
