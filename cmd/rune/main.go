package main

import (
	"flag"
	"fmt"
	"os"
	"path/filepath"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/term"

	ui "rune/pkg/ui"
	"rune/pkg/ui/styles"
	"rune/pkg/vfs"
	"rune/pkg/workspaceroot"
)

var version = "dev" // overridden by release CI: -ldflags "-X main.version=0.1.0"

func main() {
	showVersion := flag.Bool("version", false, "print version and exit")
	var workDir string
	flag.StringVar(&workDir, "w", "", "working directory (file tree root)")
	flag.Parse()

	if *showVersion {
		fmt.Println(version)
		os.Exit(0)
	}

	// Resolve file args to absolute paths relative to the shell's CWD.
	rawFiles := flag.Args()
	if len(rawFiles) > 10 {
		rawFiles = rawFiles[:10]
	}
	initialFiles := make([]string, 0, len(rawFiles))
	for _, f := range rawFiles {
		abs, err := filepath.Abs(f)
		if err != nil {
			fmt.Fprintf(os.Stderr, "bad path %q: %v\n", f, err)
			os.Exit(1)
		}
		initialFiles = append(initialFiles, abs)
	}

	var absWorkDir string
	if workDir != "" {
		abs, err := filepath.Abs(workDir)
		if err != nil {
			fmt.Fprintf(os.Stderr, "bad working dir %q: %v\n", workDir, err)
			os.Exit(1)
		}
		if _, err := os.Stat(abs); err != nil {
			fmt.Fprintf(os.Stderr, "working dir %q: %v\n", abs, err)
			os.Exit(1)
		}
		absWorkDir = abs
	} else {
		absWorkDir = resolveWorkDir()
	}

	app, err := ui.NewApp(absWorkDir, initialFiles)
	if err != nil {
		fmt.Fprintf(os.Stderr, "failed to start: %v\n", err)
		os.Exit(1)
	}

	if err := run(app); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

// resolveWorkDir picks the workspace root when -w was not given. cwd and home
// are read here via os.* — the documented §1.4.9 launch-bootstrap exception,
// same class as the -w validation above; every actual directory read during
// discovery goes through the injected vfs.Disk{} via workspaceroot.Resolve.
//
// An existing rune workspace (or -w) is the only silent outcome; otherwise,
// if stdin is a TTY, the user is prompted for where to create a new one
// (rootChooser) — a Quit exits the process cleanly. Non-interactively (e.g.
// `echo | rune`, or the session fuzzer's always-non-TTY/-w-driven harness)
// this falls back to cwd, matching prior behavior.
func resolveWorkDir() string {
	cwd, err := os.Getwd()
	if err != nil {
		fmt.Fprintf(os.Stderr, "warning: failed to get working directory: %v\n", err)
		cwd = "."
	}
	home, err := os.UserHomeDir()
	if err != nil {
		home = "" // workspaceroot.Resolve degrades gracefully: no home ceiling/candidate
	}

	res := workspaceroot.Resolve(vfs.Disk{}, cwd, home)
	if res.WorkDir != "" {
		return res.WorkDir
	}

	if !term.IsTerminal(os.Stdin.Fd()) {
		return cwd
	}

	dir, ok, err := runRootChooser(res.Prompt)
	if err != nil {
		// The chooser could not run (e.g. a terminal error) — this is a
		// failure, not a user quit, so fall back to cwd and keep launching
		// (matching the non-interactive path) rather than exiting.
		fmt.Fprintf(os.Stderr, "warning: workspace chooser failed: %v; using %s\n", err, cwd)
		return cwd
	}
	if !ok {
		os.Exit(0) // the user deliberately quit the chooser (Esc/Ctrl+C)
	}
	return dir
}

// runRootChooser runs the chooser to completion as its own tea.Program. It
// returns the user's pick (ok=true), ok=false if they quit (Esc/Ctrl+C), or a
// non-nil err if the program itself failed to run — the caller distinguishes a
// quit (exit cleanly) from a failure (fall back and keep launching).
func runRootChooser(prompt *workspaceroot.Prompt) (dir string, ok bool, err error) {
	m := newRootChooser(prompt, styles.Default())
	final, err := tea.NewProgram(m).Run()
	if err != nil {
		return "", false, err
	}
	dir, ok = final.(rootChooser).Chosen()
	return dir, ok, nil
}
