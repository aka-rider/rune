package chat

import (
	"context"
	"path/filepath"
	"strings"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"rune/pkg/ai"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
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
	input       string
	loading     bool
	errMsg      string
	filePath    string
	fileContent string
	width       int
	height      int
	focused     bool
	scrollOff   int
	client           ai.Client
	cancelReq        context.CancelFunc
	initErr          string
	dictationPartial string
	keys             keymap.Bindings
	styles           styles.Styles
}

// New constructs a Model. It attempts to initialise the AI client from
// environment variables; if that fails the error is stored and displayed
// inside the pane rather than crashing the application.
func New(keys keymap.Bindings, st styles.Styles) Model {
	m := Model{keys: keys, styles: st}
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

// SetSize stores the allocated dimensions (inner content area).
func (m Model) SetSize(w, h int) Model {
	m.width = w
	m.height = h
	return m
}

// Height returns the allocated height.
func (m Model) Height() int { return m.height }

// SetFocused sets keyboard focus.
func (m Model) SetFocused(f bool) Model {
	m.focused = f
	return m
}

// SetDictationPartial shows provisional dictation text alongside the committed input.
func (m Model) SetDictationPartial(text string) Model { m.dictationPartial = text; return m }

// FinalizeDictation appends the final dictation text to input and clears the provisional display.
func (m Model) FinalizeDictation(text string) Model {
	m.input += text
	m.dictationPartial = ""
	return m
}

// CancelDictation clears provisional dictation text without committing anything.
func (m Model) CancelDictation() Model { m.dictationPartial = ""; return m }

// SetFileContext updates the file path and content used as the system prompt.
func (m Model) SetFileContext(path, content string) Model {
	m.filePath = path
	m.fileContent = content
	return m
}

// Update handles messages and key input.
func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
	switch msg := msg.(type) {
	case responseMsg:
		m.loading = false
		m.messages = append(m.messages, chatMessage{role: "assistant", content: msg.content})
		return m, nil

	case responseErrMsg:
		m.loading = false
		m.errMsg = msg.err.Error()
		return m, nil

	case tea.KeyPressMsg:
		if !m.focused {
			return m, nil
		}
		switch {
		case msg.Code == tea.KeyEnter && msg.Mod == 0:
			return m.submit()

		case msg.Code == tea.KeyBackspace && msg.Mod == 0:
			if len(m.input) > 0 {
				runes := []rune(m.input)
				m.input = string(runes[:len(runes)-1])
			}

		case msg.Code == tea.KeyUp && msg.Mod == 0:
			m.scrollOff++

		case msg.Code == tea.KeyDown && msg.Mod == 0:
			if m.scrollOff > 0 {
				m.scrollOff--
			}

		case msg.Code == tea.KeyPgUp:
			m.scrollOff += max(1, m.height/2)

		case msg.Code == tea.KeyPgDown:
			m.scrollOff -= max(1, m.height/2)
			if m.scrollOff < 0 {
				m.scrollOff = 0
			}

		default:
			// Printable character input.
			// msg.Text covers the Kitty keyboard protocol (including Shift for
			// uppercase). The fallback handles legacy terminals.
			text := msg.Text
			if text == "" && msg.Mod == 0 && msg.Code >= ' ' && msg.Code <= '~' {
				text = string(msg.Code)
			}
			if text != "" {
				m.input += text
			}
		}
	}
	return m, nil
}

// submit sends the current input to the AI API.
func (m Model) submit() (Model, tea.Cmd) {
	if strings.TrimSpace(m.input) == "" || m.loading {
		return m, nil
	}
	text := m.input
	m.input = ""
	m.messages = append(m.messages, chatMessage{role: "user", content: text})
	m.loading = true
	m.scrollOff = 0
	m.errMsg = ""

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

	// Divider + input (2 lines at the bottom).
	divider := m.styles.ChatDivider.Render(strings.Repeat("─", m.width))
	cursor := ""
	if m.focused {
		cursor = "█"
	}
	partialSuffix := ""
	if m.dictationPartial != "" {
		partialSuffix = m.styles.FooterMeta.Render(m.dictationPartial)
	}
	inputLine := m.styles.ChatInput.Render("❯ "+m.input+cursor) + partialSuffix

	// Messages area: total height minus the 3 chrome lines (title, divider, input).
	availH := m.height - 3
	if availH < 0 {
		availH = 0
	}

	var msgArea string
	if m.initErr != "" {
		msgArea = m.styles.Error.Width(m.width).Render("Config error: " + m.initErr)
	} else {
		msgArea = m.renderMessages(availH)
	}

	full := lipgloss.JoinVertical(lipgloss.Left,
		titleLine,
		msgArea,
		divider,
		inputLine,
	)
	return lipgloss.NewStyle().MaxWidth(m.width).MaxHeight(m.height).Render(full)
}

// renderMessages renders the conversation history into a fixed-height block.
func (m Model) renderMessages(availH int) string {
	var parts []string

	for _, cm := range m.messages {
		switch cm.role {
		case "user":
			parts = append(parts, m.styles.ChatUserMsg.Width(m.width).Render("You: "+cm.content))
		default:
			parts = append(parts, m.styles.ChatAssistantMsg.Width(m.width).Render(cm.content))
		}
	}

	if m.loading {
		parts = append(parts, m.styles.ChatLoading.Render("thinking…"))
	}
	if m.errMsg != "" {
		parts = append(parts, m.styles.Error.Render(m.errMsg))
	}

	block := strings.Join(parts, "\n")
	allLines := strings.Split(block, "\n")

	total := len(allLines)
	end := total - m.scrollOff
	if end < 0 {
		end = 0
	}
	if end > total {
		end = total
	}
	start := end - availH
	if start < 0 {
		start = 0
	}
	visible := allLines[start:end]

	// Pad to availH so the divider stays at a fixed position.
	for len(visible) < availH {
		visible = append(visible, "")
	}

	return strings.Join(visible, "\n")
}
