# mcp-doze

[English](README.md) | [日本語](README.ja.md)

**ローカルMCPサーバーを、使うときに起動。**

MCPサーバーをいくつも登録している環境で、使わないローカルサーバーの常駐を減らします。`mcp-doze`はツール一覧をキャッシュから返し、実際の操作が届いたときにサーバーを起動するコマンドラッパーです。

起動後は同じプロセスを使い、クライアントの接続が閉じるまで状態を保持します。既存の起動コマンドの前に挟んで使えます。

## 起動コマンドの変更例

```diff
-node /absolute/path/to/server.mjs
+mcp-doze run --auto-refresh -- node /absolute/path/to/server.mjs
```

サーバーは既存のMCPクライアントに登録したまま使えます。起動設定・認証情報・承認方針も、そのクライアントで管理します。

## インストール

### Homebrew（macOS）

Homebrew 7以降では、tapを追加し、mcp-dozeのFormulaを信頼対象として登録してからインストールします。

```sh
brew tap mmng-a/tap
brew trust --formula mmng-a/tap/mcp-doze
brew install mmng-a/tap/mcp-doze
```

[公開tap](https://github.com/mmng-a/homebrew-tap)からソースをビルドします。ビルドに必要なRustはHomebrewが導入します。配布側の更新手順は[配布手順](docs/homebrew.md)を参照してください。

### ソースからビルド

ビルドにはRust 1.85以降が必要です。検証済みの環境はmacOSで、実装にはUnix APIを使用しています。Windowsには対応していません。インストール後の実行にはRust・Node.js・Pythonは不要です。中継するMCPサーバーには、そのサーバー自身の実行環境が必要です。

```sh
git clone https://github.com/mmng-a/mcp-doze.git
cd mcp-doze
cargo install --path . --locked
```

## 使い方

既存のサーバー登録を残し、起動コマンドの前に`mcp-doze run --auto-refresh --`を挟みます。Codexの`config.toml`では、例えば次のように設定します。

```toml
[mcp_servers.example]
command = "/absolute/path/to/mcp-doze"
args = ["run", "--auto-refresh", "--", "node", "/absolute/path/to/server.mjs"]
```

インストール先は`command -v mcp-doze`で確認できます。元の環境変数・作業ディレクトリ・承認方針・ツール許可リストは維持してください。別のサーバー登録台帳は不要です。

| タイミング | 動作 |
| --- | --- |
| キャッシュがない、または更新が必要 | 一時的にサーバーを起動して一覧情報を取得し、停止する |
| 接続時の初期化・一覧取得 | キャッシュから応答する |
| 最初の操作 | サーバーを起動し、初期化結果と一覧情報を確認する |
| 以降の操作 | 同じサーバーを使い、元のJSONをそのまま中継する |
| クライアントが接続を解放 | 管理対象のサーバープロセス群を終了する |

新しい作業ディレクトリでの初回接続では、キャッシュ取得が必要になる場合があります。一覧を表示できても、認証が確認済みとは限りません。

## コマンド

```text
mcp-doze run [OPTIONS] -- COMMAND [ARG...]
mcp-doze cache refresh [OPTIONS] -- COMMAND [ARG...]
mcp-doze cache inspect [OPTIONS] -- COMMAND [ARG...]
mcp-doze codex sync-plugins [--codex-home PATH]

--auto-refresh         キャッシュがない場合や更新が必要な場合に自動取得
--cache-dir PATH       キャッシュの保存先を変更
--cache-env NAME       キャッシュの識別に使う環境変数。複数指定可
--fingerprint PATH     更新を検出する追加スクリプトやマニフェスト。複数指定可
--repl-cleanup         サーバー起動前の検証済みturn_endedへ代理応答
--timeout-ms N         初期化と個々の書き込みの制限時間。既定は10000ミリ秒
```

`--`以降はシェルを介さず、そのままサーバーへ渡します。`--auto-refresh`は`run`だけに作用します。指定しない場合、キャッシュを使えなければエラーと更新方法を返します。

明示的にキャッシュを取得する場合は、クライアントと同じコマンド・作業ディレクトリ・環境変数・キャッシュ関連オプションを使います。

```sh
mcp-doze cache refresh -- node /absolute/path/to/server.mjs
mcp-doze cache inspect -- node /absolute/path/to/server.mjs
```

### キャッシュの識別と保存

キャッシュの識別には、実行ファイル・引数・作業ディレクトリ・引数で指定したファイルのメタデータ・追加のフィンガープリント・指定した環境変数・終了通知の処理方針を使います。継承した環境変数はそのまま中継しますが、キャッシュの識別に使うのは`--cache-env`で指定したものだけです。ツール一覧や指示文を変える変数を指定してください。値はハッシュ化し、平文では保存しません。

間接的に読み込むコードやマニフェストには`--fingerprint`を使います。ディレクトリを指定した場合は、そのディレクトリ自体を識別し、配下の全ファイルは追跡しません。

既定の保存先は、macOSでは`~/Library/Caches/mcp-doze`、その他のUnix環境では`~/.cache/mcp-doze`です。`XDG_CACHE_HOME`の指定があれば優先します。ディレクトリは実行ユーザー所有・権限`0700`、ファイルは権限`0600`が必要です。

### REPLの終了通知

REPLを使っていなくても、ホストが`turn_ended`ツールを呼ぶことがあります。その通知だけで本体を起動しないようにするには、`--repl-cleanup`を追加します。

```sh
mcp-doze run --auto-refresh --repl-cleanup -- /absolute/path/to/repl-server
```

このオプションは、`hook_event_name`・`session_id`・`turn_id`の3つだけを必須の空でない文字列として受け取る、検証済みのスキーマを対象とします。異なるスキーマは拒否します。起動前はdozeが空の終了処理結果を返し、起動後は実サーバーが処理します。リセット操作では必ず実サーバーを起動します。

## 認証とエラー

認証情報は、MCPクライアント・サーバー・接続先が従来使用している保存先で管理します。dozeは認証情報の保管やOAuthログインを行いません。HTTP接続のMCPは中継対象外です。

初期化や一覧取得のエラーは、コードと詳細をクライアントへ返します。ツールのエラーもそのまま中継します。認証エラーや実行結果はキャッシュしません。エラーやログイン案内の表示方法はクライアント側が決めます。

最初の操作を中継する前に、実サーバーの初期化結果と一覧をキャッシュと比較します。不一致ならキャッシュを無効化し、その操作を実行せずに接続を閉じます。サーバーの異常終了や不正な通信形式も接続終了の対象です。操作の自動再試行や、状態を失ったREPLの自動再起動は行いません。

## Codexプラグイン

プラグインの識別情報・認証方針・スキル・フック定義を維持するため、同梱MCPは元のプラグイン内でラップします。

```sh
mcp-doze codex sync-plugins
```

この明示的なコマンドは、`CODEX_HOME`（既定は`~/.codex`）内の`plugins/cache/<marketplace>/<plugin>/<version>/.mcp.json`を調べ、明示的に無効化されていないstdio定義の`command`と`args`だけを変更します。HTTP定義とラップ済みのサーバーは変更しません。CUAには`--repl-cleanup`を追加します。元の定義は、同じCodexホーム内の`backups/mcp-doze/plugins`へ非公開のバックアップとして保存します。

Codexの正式な起動コマンド上書き設定ではなく、ダウンロード済みのプラグインキャッシュを変更する方式です。プラグイン更新で置き換わる場合があるため、Codex起動前に再実行してください。`PATH`で実際のCodexより先に見つかる起動スクリプトで自動化できます。

```sh
#!/bin/sh
set -eu
/absolute/path/to/mcp-doze codex sync-plugins >/dev/null
exec /absolute/path/to/the/real/codex "$@"
```

起動スクリプトと実際のCodexには別のパスを指定してください。同期に失敗した場合は、エラーを表示して起動を止めます。既存のCodex app-serverは終了するまで古い起動定義を保持する場合があります。dozeがそれを再起動したり、進行中の作業を中断したりすることはありません。

## Xcode

Xcodeは、`mcpbridge`の直接の親プロセスのコード署名からエージェントを識別します。dozeを別のエージェントとして、Xcodeの通常の手順で承認してください。更新後も識別を維持するには、同じ署名IDと識別子でインストール済みバイナリへ署名します。

```sh
security find-identity -v -p codesigning
codesign --force --sign "YOUR_CODE_SIGNING_IDENTITY" \
  --identifier io.github.mmng-a.mcp-doze --timestamp=none /absolute/path/to/mcp-doze
```

`XcodeListWorkspaces`は、承認要求を出さずに未承認エラーを返す場合があります。`XcodeOpenWorkspace`はアクセスを要求し、承認後に指定したワークスペースを開きます。画面を使わないXcodeでは、`xcrun mcp-server status`で保留中の要求を確認し、該当する要求をXcodeの正式なコマンドで承認します。dozeがXcodeの承認データベースを変更することはありません。

## 接続の終了と対応範囲

- 接続ごとにサーバーを分離します。サーバー共有・常駐デーモン・アイドル時間による終了はありません。
- ターンの終了だけではサーバーは止まりません。ホストによるMCP接続の解放が必要です。端末を閉じても、共有app-server内に接続が残る場合があります。
- EOF・終了シグナル・回復できないエラーで終了処理を開始します。サーバーへの入力を閉じ、TERMを送り、2秒後にKILLを送ります。最後のKILLにはdoze自身も含むため、通常の終了処理でもシグナルによる終了になる場合があります。
- 切り離されたプロセス・既存のアプリ・外部サービスは管理対象外です。dozeだけを処理不能なSIGKILLで終了すると、起動済みのサーバーが残る場合があります。
- ローカルstdioのJSON-RPCが対象です。現在の一覧取得は`initialize`を使用し、`2026-07-28`仕様の`server/discover`方式には未対応です。
- 一覧情報をキャッシュできるサーバーが対象です。接続ごとに一覧が変わる場合や、一覧取得に対話が必要な場合は、別途互換性の確認が必要です。
- 1メッセージと起動待ち入力の上限は、それぞれ64 MiBです。上限付きの入出力チャネルには、追加のメッセージが保持される場合があります。
- ホスト側のツール制限時間には、遅延起動にかかる時間も含まれます。再接続では新しいサーバーを起動し、以前の状態は失われます。

## 開発

`mcp-doze`はRustで実装しています。接続ごとにサーバープロセスと状態を分離します。

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
```

テストはRustとローカルのMCPテスト用サーバーで実行します。Python・外部サービス・デスクトップアプリは不要です。実装の境界と互換性の確認結果は、[設計](docs/design.md)と[検証](docs/validation.md)を参照してください。

## ライセンス

[MIT](LICENSE)。[lazy-mcp-wrapper](https://github.com/sleticalboy/lazy-mcp-wrapper)を設計の参考にした、独立したRust実装です。
