# mcp-doze

[English](README.md) | [日本語](README.ja.md)

**使っていないMCPサーバーのリソース消費を減らしませんか？**

Codexでは、[サブエージェントにも親のMCP設定が引き継がれます](https://learn.chatgpt.com/docs/agent-configuration/subagents)。同じローカルMCPサーバーが、複数のチャットやサブエージェントで個別に起動されることがあるのは、このためです。

1つあたりは数十MBでも、何十組も起動すれば合計はGB単位になります。ツール一覧を取得するだけでもサーバーが起動するため、実際には使っていないツールにもメモリが必要です。

mcp-dozeは、初期化や一覧取得にはキャッシュから応答し、実際の操作が届いたときにサーバーを起動するラッパーです。MCPを登録したまま未使用サーバーの常駐を減らせるので、使うたびに設定を切り替える必要はありません。Codexプラグインに同梱されたローカルMCPにも対応しています。

## インストール

macOSで動作を検証しています。実装はUnix向けで、Windowsには対応していません。

Homebrew 7以降では、次のコマンドでtapを追加し、mcp-dozeのFormulaを信頼対象に登録してからインストールしてください。

```sh
brew tap mmng-a/tap
brew trust --formula mmng-a/tap/mcp-doze
brew install mmng-a/tap/mcp-doze
```

## 使い方

既存のMCP登録を、mcp-doze経由でサーバーを起動する設定に変更してください。次は、Codexの`config.toml`でNode.js製のサーバーを登録する例です。

```toml
[mcp_servers.example]
command = "/absolute/path/to/mcp-doze"
args = ["run", "--auto-refresh", "--", "node", "/absolute/path/to/server.mjs"]
```

mcp-dozeのインストール先は`command -v mcp-doze`で確認できます。元の環境変数・作業ディレクトリ・承認方針・ツール許可リストは変更せず、起動コマンドと引数を上の形式に置き換えてください。

## Codexプラグイン

プラグインに同梱されたMCPにも適用する場合は、Codexを起動する前に次のコマンドを実行してください。

```sh
mcp-doze codex sync-plugins
```

元のプラグインの起動定義を変更し、プラグインの識別情報・認証方針・スキル・フック定義を維持します。プラグイン更新によって起動定義が置き換わる場合があるため、Codexの起動前に再実行する必要があります。[詳しい適用方法](docs/usage.md#codex-plugins)も参照してください。

## 注意点

- 対象はローカルのstdio接続です。HTTP接続のMCPには対応していません。
- キャッシュがない場合や更新が必要な場合は、一覧情報を取得するため、一時的に実サーバーを起動します。
- dozeはMCP接続ごとに1プロセス起動します。使用を開始した実サーバーは、クライアントが接続を閉じるまで状態を保持するため、待機時間による自動停止はありません。

## ドキュメント

詳細な設定や測定結果は、以下の英語の資料を参照してください。

- [設定と動作](docs/usage.md)：オプション・キャッシュ・認証・プラグイン対応
- [設計](docs/design.md)・[検証結果とメモリ測定](docs/validation.md)
- [ソースからのビルドと開発](docs/development.md)・[Homebrew配布手順](docs/homebrew.md)

## ライセンス

[MIT](LICENSE)。設計は[lazy-mcp-wrapper](https://github.com/sleticalboy/lazy-mcp-wrapper)を参考にしています。
