Output assets/l-vis-demo.gif

Require cargo
Require ffmpeg
Require ttyd

Set Shell "bash"
Set Theme "CyberPunk2077"
Set Width 1600
Set Height 900
Set FontSize 18
Set Framerate 30
Set PlaybackSpeed 0.5
Set TypingSpeed 12ms
Env TERM xterm-256color
Env COLORTERM truecolor

Hide
Type "unset NO_COLOR"
Enter
Type "clear"
Enter
Show

Type "./target/debug/l-vis --demo --theme dark tui"
Enter
Wait+Screen /L-VIS/
Sleep 1500ms

Type "2"
Sleep 2500ms

Type "5"
Sleep 2500ms

Type "t"
Sleep 2s

Type "4"
Sleep 2500ms

Type "1"
Sleep 2s

Type "q"
