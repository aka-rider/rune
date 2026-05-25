package editor

type FileLoadedMsg struct {
        Path    string
        Content []byte
}

type FileLoadErrorMsg struct {
        Path string
        Err  error
}

type FileClosedMsg struct {
        Path string
}

type FileSavedMsg struct {
        Path             string
        RequestID        string
        SavedContentHash string
}

type FileSaveErrorMsg struct {
        Path      string
        RequestID string
        Err       error
}

type ContentChangedMsg struct {
        Path  string
        Dirty bool
}
