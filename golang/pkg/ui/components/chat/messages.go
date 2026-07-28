package chat

// responseMsg carries the assistant's reply from a successful API call.
type responseMsg struct{ content string }

// responseErrMsg carries an error from a failed API call.
type responseErrMsg struct{ err error }
