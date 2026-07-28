package textedit

import "rune/pkg/command"

// Find command stubs — these exist solely so app.go startup verification
// finds every keybind's command name. The workspace intercepts find.*
// keybindings (FindOpen/FindNext/FindPrev) before they reach the editor.

func execFindStub(ctx command.CommandContext) command.Result {
	return noneResult()
}

var findSpecs = []cmdSpec{
	{name: "find.open", when: "editorFocused", exec: execFindStub},
	{name: "find.close", when: "editorFocused", exec: execFindStub},
	{name: "find.replace-open", when: "editorFocused", exec: execFindStub},
	{name: "find.replace", when: "editorFocused", exec: execFindStub},
	{name: "find.replace-all", when: "editorFocused", exec: execFindStub},
	{name: "find.next", when: "editorFocused", exec: execFindStub},
	{name: "find.previous", when: "editorFocused", exec: execFindStub},
}
