# shellcheck shell=bash
type_text 0.07 'rune README.md'
keys 1 Enter
wait_for_text "Good News Everyone" 60
sleep 3

keys 2 PageDown
keys 2 PageDown
keys 2 PageUp
keys 1 Down
keys 1 Down
keys 2 Down
keys 2 Up
keys 2 PageUp
keys 2 PageUp
keys 1 C-Home

keys 1.5 C-o
type_text 0.25 'cla'
keys 2 Enter
sleep 2

for _ in 1 2 3 4 5 6; do keys 0.4 PageDown; done
keys 1 C-End
keys 1 End
keys 0.5 Enter
keys 0.8 Enter
type_text 0.06 '| Feature | Status |'
keys 0.5 Enter
type_text 0.06 '| --- | --- |'
keys 0.5 Enter
type_text 0.06 '| Crash recovery | durable |'
keys 0.5 Enter
type_text 0.06 '| Merge resolver | built in |'
keys 0.5 Enter
keys 1 Enter
sleep 4

keys 2 C-b
keys 2 Down
keys 2 Enter
sleep 2
keys 2 C-t
keys 1.5 Up
keys 1.5 Up
keys 1.5 Down
keys 1.5 Down
keys 2 Enter
keys 2 C-b

keys 1 Down
keys 1 Down
keys 1 End
type_text 0.06 ' Edited here in rune.'
sleep 1.5
sed -i '3s/.*/A second file, rewritten by another program./' "$WORKSPACE/notes.md"
keys 1 C-s
wait_for_text "changed on disk" 40
sleep 3
keys 2 -l 'm'
wait_for_text "conflict" 40
sleep 4
keys 2 C-k
keys 2 Escape
sleep 2

keys 2 C-t
keys 1.5 Up
keys 1.5 Up
keys 2 Enter
keys 1 C-b
keys 1 End
keys 0.8 Enter
type_text 0.07 'These words survive a crash.'
sleep 3

kill -9 "$(pane_child_pid)"
wait_for_text "$SHELL_PROMPT" 40
sleep 2
type_text 0.06 'clear'
keys 1 Enter
type_text 0.07 'rune README.md'
keys 2 Enter
wait_for_text "These words survive a crash." 60
keys 2 Down
keys 2 Down
keys 2 Down
keys 2 Up
keys 2 Up
sleep 2
