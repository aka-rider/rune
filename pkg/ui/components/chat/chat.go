package chat

import (
	"context"
	"path/filepath"
	"strings"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"rune/pkg/ai"
	"rune/pkg/command"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/keybind"
	"rune/pkg/terminal"
	"rune/pkg/ui/components/markdownedit"
	"rune/pkg/ui/components/textedit"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

// chatFocus tracks which sub-component (prompt or display) has keyboard focus.
type chatFocus int

const (
	chatFocusPrompt  chatFocus = iota // default: typing into the input
	chatFocusDisplay                  // navigating / copying from the conversation
)

// chatMessage is a single entry in the in-session conversation history.
type chatMessage struct {
	role    string // "user" | "assistant"
	content string
}

// Model is the chat right-pane component.
// All fields are value types except cancelReq (context.CancelFunc), which is
// explicitly permitted by §6.3 of the engineering standard.
type Model struct {
	messages    []chatMessage
	loading     bool
	errMsg      string
	filePath    string
	fileContent string
	width       int
	height      int
	focused     bool
	subFocus    chatFocus
	display     markdownedit.Model // read-only conversation view (D9)
	prompt      textedit.Model     // multi-line input (D15)
	client      ai.Client
	cancelReq   context.CancelFunc
	initErr     string
	keys        keymap.Bindings
	styles      styles.Styles
}

// New constructs a Model. It attempts to initialise the AI client from
// environment variables; if that fails the error is stored and displayed
// inside the pane rather than crashing the application.
func New(keys keymap.Bindings, st styles.Styles, reg command.Registry, resolver keybind.Resolver, caps terminal.TermCaps) Model {
	m := Model{keys: keys, styles: st}
	// Conversation display: read-only markdownedit for full markdown rendering (D9).
	m.display = markdownedit.New(keys, st, caps,
		markdownedit.WithRegistry(reg),
		markdownedit.WithResolver(resolver),
	).SetReadOnly(true)
	// Prompt: plain textedit for multi-line input (D15).
	m.prompt = textedit.New(keys, st,
		textedit.WithSyncFunc(textedit.PlainSync),
		textedit.WithRegistry(reg),
		textedit.WithResolver(resolver),
	)
	c, err := ai.NewClient()
	if err != nil {
		m.initErr = err.Error()
	} else {
		m.client = c
	}
	return m
}

// Init implements the component contract. Chat has no startup commands.
func (m Model) Init() tea.Cmd { return nil }

// SetSize stores the allocated dimensions and redistributes them to sub-components.
func (m Model) SetSize(w, h int) Model {
	m.width = w
	m.height = h
	return m.recalcLayout()
}

// Height returns the allocated height.
func (m Model) Height() int { return m.height }

// SetFocused sets keyboard focus and routes it to the active sub-component.
func (m Model) SetFocused(f bool) Model {
	m.focused = f
	m.prompt = m.prompt.SetFocused(f && m.subFocus == chatFocusPrompt)
	m.display = m.display.SetFocused(f && m.subFocus == chatFocusDisplay)
	return m
}

func (m Model) Focused() bool { return m.focused }

// DrainEdits forwards to the prompt, returning chat.Model (only prompt is journaled).
func (m Model) DrainEdits() (Model, []buffer.AppliedEdit) {
	var edits []buffer.AppliedEdit
	m.prompt, edits = m.prompt.DrainEdits()
	return m, edits
}

// Cursors returns the cursor state of the prompt.
func (m Model) Cursors() []cursor.Cursor {
	return m.prompt.Cursors()
}

// ApplyInverse applies inverse edits to the prompt (workspace-driven undo).
func (m Model) ApplyInverse(edits []buffer.AppliedEdit) Model {
	m.prompt = m.prompt.ApplyInverse(edits)
	return m
}

// Reapply applies edits forward to the prompt (workspace-driven redo).
func (m Model) Reapply(edits []buffer.AppliedEdit) Model {
	m.prompt = m.prompt.Reapply(edits)
	return m
}

// SetCursors restores cursor state on the prompt.
func (m Model) SetCursors(cs []cursor.Cursor) Model {
	m.prompt = m.prompt.SetCursors(cs)
	return m
}

// PromptContent returns the current prompt text.
func (m Model) PromptContent() string {
	return m.prompt.Content()
}

// SetPromptContent replaces the prompt text (for draft restoration).
func (m Model) SetPromptContent(s string) Model {
	m.prompt = m.prompt.SetContent(s)
	return m
}

// ApplyToPrompt replaces the range [start, end) in the prompt with text.
// Called by the workspace to route dictation chunks (D16).
func (m Model) ApplyToPrompt(start, end int, text string) Model {
	m.prompt = m.prompt.ReplaceRange(start, end, text)
	return m.recalcLayout()
}

// SetFileContext updates the file path and content used as the system prompt.
func (m Model) SetFileContext(path, content string) Model {
	m.filePath = path
	m.fileContent = content
	return m
}

// Update handles messages and key input.
func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
	var cmds []tea.Cmd
	var cmd tea.Cmd

	switch msg := msg.(type) {
	case responseMsg:
		m.loading = false
		m.messages = append(m.messages, chatMessage{role: "assistant", content: msg.content})
		m = m.rebuildDisplay()
		return m, nil

	case responseErrMsg:
		m.loading = false
		m.errMsg = msg.err.Error()
		m = m.rebuildDisplay()
		return m, nil

	case tea.KeyPressMsg:
		// Enter (no modifier) submits the prompt. Shift+Enter falls through to
		// textedit which inserts a newline.
		if msg.Code == tea.KeyEnter && msg.Mod == 0 {
			return m.submit()
		}
		// Route to the active sub-component.
		switch m.subFocus {
		case chatFocusPrompt:
			m.prompt, cmd = m.prompt.Update(msg)
			cmds = append(cmds, cmd)
			m = m.recalcLayout() // prompt height may have changed
		case chatFocusDisplay:
			m.display, cmd = m.display.Update(msg)
			cmds = append(cmds, cmd)
		}
		return m, tea.Batch(cmds...)
	}

	// Non-key messages: forward to both sub-components (timers, image events, etc.)
	m.display, cmd = m.display.Update(msg)
	cmds = append(cmds, cmd)
	m.prompt, cmd = m.prompt.Update(msg)
	cmds = append(cmds, cmd)

	return m, tea.Batch(cmds...)
}

// submit sends the current prompt content to the AI API.
func (m Model) submit() (Model, tea.Cmd) {
	text := strings.TrimSpace(m.prompt.Content())
	if text == "" || m.loading {
		return m, nil
	}
	m.prompt = m.prompt.SetContent("")
	m.messages = append(m.messages, chatMessage{role: "user", content: text})
	m.loading = true
	m.errMsg = ""
	m = m.rebuildDisplay()
	m = m.recalcLayout()

	if m.cancelReq != nil {
		m.cancelReq()
	}
	ctx, cancel := context.WithCancel(context.Background())
	m.cancelReq = cancel

	// Capture locals for closure safety (§5.5): never capture m.field inside a Cmd.
	client := m.client
	apiMsgs := m.buildAPIMessages()

	return m, func() tea.Msg {
		content, err := client.Chat(ctx, apiMsgs)
		if err != nil {
			return responseErrMsg{err: err}
		}
		return responseMsg{content: content}
	}
}

// buildAPIMessages constructs the message slice for the API call.
func (m Model) buildAPIMessages() []ai.Message {
	var msgs []ai.Message
	if m.filePath != "" {
		system := "You are a helpful assistant.\n\nFile: " + m.filePath + "\n\nContent:\n" + m.fileContent
		msgs = append(msgs, ai.Message{Role: "system", Content: system})
	}
	for _, cm := range m.messages {
		msgs = append(msgs, ai.Message{Role: cm.role, Content: cm.content})
	}
	return msgs
}

// rebuildDisplay reconstructs the display buffer from the message history.
// Must be called after any change to m.messages, m.loading, or m.errMsg.
func (m Model) rebuildDisplay() Model {
	var parts []string
	for _, cm := range m.messages {
		switch cm.role {
		case "user":
			parts = append(parts, "**You:** "+cm.content)
		default:
			parts = append(parts, cm.content)
		}
		parts = append(parts, "")
	}
	if m.loading {
		parts = append(parts, "*thinking…*")
	}
	if m.errMsg != "" {
		parts = append(parts, "**Error:** "+m.errMsg)
	}

	content := strings.Join(parts, "\n")
	wasAtBottom := m.display.AtBottom()
	m.display = m.display.SetContent(content)
	if wasAtBottom {
		m.display = m.display.GotoBottom()
	}
	return m
}

// recalcLayout distributes allocated dimensions to display and prompt.
// Called from SetSize and any time prompt content changes height.
func (m Model) recalcLayout() Model {
	if m.width == 0 || m.height == 0 {
		return m
	}
	const titleH = 1
	const dividerH = 1
	availH := m.height - titleH - dividerH
	if availH < 0 {
		availH = 0
	}

	// Prompt height: natural content height clamped to [3, 50%].
	rawPromptH := m.prompt.NaturalContentHeight(m.width)
	promptH := rawPromptH
	if promptH < 3 {
		promptH = 3
	}
	maxPromptH := availH / 2
	if maxPromptH < 3 {
		maxPromptH = 3
	}
	if promptH > maxPromptH {
		promptH = maxPromptH
	}

	displayH := availH - promptH
	if displayH < 0 {
		displayH = 0
		promptH = availH
	}

	// Use relative coordinates; absolute screen Y is not available here.
	// Inline images will use Y=0 as base, which is acceptable since the chat
	// display renders as a string composed into the workspace border (no iTerm2
	// absolute-position escapes expected in read-only conversation display).
	m.display = m.display.SetRect(textedit.Rect{X: 0, Y: titleH, W: m.width, H: displayH})
	m.prompt = m.prompt.SetRect(textedit.Rect{X: 0, Y: titleH + displayH + dividerH, W: m.width, H: promptH})
	return m
}

// View renders the chat pane. It is a pure function of model state.
func (m Model) View() string {
	if m.width == 0 || m.height == 0 {
		return ""
	}

	// Title line.
	title := "Chat"
	if m.filePath != "" {
		title = "Thread: " + filepath.Base(m.filePath)
	}
	titleLine := m.styles.ChatTitle.Width(m.width).Render(title)

	// Divider between display and prompt.
	divider := m.styles.ChatDivider.Render(strings.Repeat("─", m.width))

	// Init error takes priority.
	if m.initErr != "" {
		errBlock := m.styles.Error.Width(m.width).Render("Config error: " + m.initErr)
		full := lipgloss.JoinVertical(lipgloss.Left, titleLine, errBlock, divider, "")
		return lipgloss.NewStyle().MaxWidth(m.width).MaxHeight(m.height).Render(full)
	}

	full := lipgloss.JoinVertical(lipgloss.Left,
		titleLine,
		m.display.View(),
		divider,
		m.prompt.View(),
	)
	return lipgloss.NewStyle().MaxWidth(m.width).MaxHeight(m.height).Render(full)
}
