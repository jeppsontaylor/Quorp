Output assets/images/quorp_tui_benchmark.gif

Require bash
Require git

Set Shell "bash"
Set FontSize 18
Set FontFamily "Menlo"
Set LetterSpacing 0
Set Width 1280
Set Height 720
Set Padding 18
Set Framerate 10
Set PlaybackSpeed 1.65
Set Theme "TokyoNight"
Set WindowBar ColorfulRight
Set WindowBarSize 34
Set TypingSpeed 12ms

Hide
Type "cd $(git rev-parse --show-toplevel) && clear" Enter
Show

Type "bash assets/recordings/readme-benchmark-demo.sh" Enter
Sleep 9s
