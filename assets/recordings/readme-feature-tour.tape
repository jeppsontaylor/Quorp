Output assets/images/quorp_tui_feature_tour.gif

Require bash
Require git
Require sed

Set Shell "bash"
Set FontSize 18
Set FontFamily "Menlo"
Set LetterSpacing 0
Set Width 1280
Set Height 720
Set Padding 18
Set Framerate 12
Set PlaybackSpeed 1.35
Set Theme "TokyoNight"
Set WindowBar ColorfulRight
Set WindowBarSize 34
Set TypingSpeed 18ms

Hide
Type "cd $(git rev-parse --show-toplevel) && clear" Enter
Show

Type "./target/debug/quorp render-demo" Enter
Sleep 4s

Ctrl+L
Sleep 350ms
Type "./target/debug/quorp doctor" Enter
Sleep 2s

Type "./target/debug/quorp commands | sed -n '1,18p'" Enter
Sleep 2s

Type "./target/debug/quorp scan --symbols --workspace ." Enter
Sleep 2s

Type "./target/debug/quorp index status" Enter
Sleep 2s

Type "./target/debug/quorp permissions --mode auto-safe --tool run_command --command 'cargo test' --allow-command 'cargo *'" Enter
Sleep 3s
