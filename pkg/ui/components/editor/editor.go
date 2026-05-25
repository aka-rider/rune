package editor

import (
        "crypto/sha256"
        "encoding/hex"
        "fmt"
        "time"

        tea "charm.land/bubbletea/v2"
        "charm.land/lipgloss/v2"

        "rune/pkg/command"
        "rune/pkg/editor/buffer"
        "rune/pkg/editor/cursor"
        "rune/pkg/editor/display"
        "rune/pkg/editor/history"
        "rune/pkg/editor/keybind"
        "rune/pkg/ui/components/breadcrumb"
        "rune/pkg/ui/keymap"
        "rune/pkg/ui/styles"
)

type ViewportState struct {
        TopRow    int
        ScrollCol int
}

type IndentConfig struct {
        UseTabs bool
        TabSize int
}

type SaveIdentity struct {
        Path        string
        RequestID   string
        ContentHash string
        InFlight    bool
}

type CursorInfo struct {
        Line         int
        Col          int
        WordCount    int
        Dirty        bool
        ChordPending string
}

type Model struct {
        buf              buffer.Buffer
        cursors          cursor.CursorSet
        history          history.UndoStack
        dirty            bool
        savedContentHash string
        activeSave       SaveIdentity
        filePath         string
        softWrap         bool
        indent           IndentConfig

        syntaxMap  display.SyntaxMap
        wrapMap    display.WrapMap
        snapshot   display.DisplaySnapshot
        syntaxSnap display.SyntaxSnapshot
        wrapSnap   display.WrapSnapshot

        resolver keybind.Resolver
        registry command.Registry

        viewport   ViewportState
        breadcrumb breadcrumb.Model
        keys       keymap.Bindings
        styles     styles.Styles
        width      int
        height     int
        focused    bool
}

func New(keys keymap.Bindings, st styles.Styles, reg command.Registry, resolver keybind.Resolver) Model {
        return Model{
                buf:        buffer.New(""),
                cursors:    cursor.CursorSet{},
                history:    history.New(time.Now),
                resolver:   resolver,
                registry:   reg,
                breadcrumb: breadcrumb.New(st),
                keys:       keys,
                styles:     st,
        }
}

func (m Model) Init() tea.Cmd { return nil }

func hashContent(content string) string {
        sum := sha256.Sum256([]byte(content))
        return hex.EncodeToString(sum[:])
}

func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
        var cmd tea.Cmd
        switch msg := msg.(type) {
        case FileLoadedMsg:
                b, err := buffer.FromBytes(msg.Content)
                if err == nil {
                        m.buf = b
                        m.filePath = msg.Path
                        m.cursors = cursor.CursorSet{} // simplified
                        m.savedContentHash = hashContent(m.buf.Content())
                        m.dirty = false
                }

        case FileClosedMsg:
                if msg.Path == m.filePath {
                        m.filePath = ""
                        m.buf = buffer.New("")
                        m.savedContentHash = ""
                        m.dirty = false
                }

        case FileSavedMsg:
                if m.filePath == msg.Path && m.activeSave.InFlight && m.activeSave.RequestID == msg.RequestID {
                        m.activeSave.InFlight = false
                        m.savedContentHash = msg.SavedContentHash
                        if hashContent(m.buf.Content()) == msg.SavedContentHash {
                                m.dirty = false
                        }
                }

        case FileSaveErrorMsg:
                if m.filePath == msg.Path && m.activeSave.InFlight && m.activeSave.RequestID == msg.RequestID {
                        m.activeSave.InFlight = false
                        // handle error if needed
                }

        case tea.KeyPressMsg:
                if !m.focused {
                        return m, nil
                }
                
                // TODO: Properly evaluate keybind entries
                // For now, if string matches 'cmd.s', pass file.save
                if msg.Code == 's' && msg.Mod == tea.ModMeta {
                        res := m.registry.Execute("file.save", command.CommandContext{
                                Buffer:      m.buf,
                                FilePath:    m.filePath,
                                NewRequestID: func() string { return "req-time-" + time.Now().String() },
                                HashContent: hashContent,
                        })
                        return m.dispatchOperation(res, "file.save", time.Now())
                }
        }
        return m, cmd
}

func (m Model) View() string {
        var bd string 
        if m.focused {
                bd = fmt.Sprintf("Editor content: %s", m.filePath)
        } else {
                bd = "Editor unfocused"
        }
        return lipgloss.NewStyle().MaxWidth(m.width).MaxHeight(m.height).Height(m.height).Width(m.width).Render(bd) // added explicit bounds
}

func (m Model) SetSize(w, h int) Model {
        m.width = w
        m.height = h
        m.breadcrumb = m.breadcrumb.SetSize(w, 1)
        return m
}

func (m Model) Height() int             { return m.height }
func (m Model) SetFocused(f bool) Model { m.focused = f; return m }
func (m Model) Content() string         { return m.buf.Content() }
func (m Model) IsDirty() bool           { return m.dirty }
func (m Model) FilePath() string        { return m.filePath }
func (m Model) WantsModalInput() bool   { return false }
func (m Model) StartSave() (Model, SaveIdentity, tea.Cmd) {
        req := SaveRequest{
                Path:        m.filePath,
                Content:     m.buf.Content(),
                RequestID:   fmt.Sprintf("req-%v", time.Now().UnixNano()),
                ContentHash: hashContent(m.buf.Content()),
        }
        return m.startSaveRequest(req)
}
func (m Model) CursorInfo() CursorInfo {
        return CursorInfo{Dirty: m.dirty}
}

func (m Model) SetContent(path string, content []byte) Model {
        b, err := buffer.FromBytes(content)
        if err == nil {
                m.buf = b
                m.filePath = path
                m.savedContentHash = hashContent(string(content))
                m.dirty = false
        }
        return m
}

func (m Model) SetDir(dir string) Model {
        m.breadcrumb = m.breadcrumb.SetDir(dir)
        return m
}
func (m Model) OpenPath() string { return m.filePath }

func PreferredWidth() int { return 40 }
