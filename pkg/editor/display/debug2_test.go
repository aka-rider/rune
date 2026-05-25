package display_test

import (
	"testing"
	"fmt"

	"github.com/yuin/goldmark"
	"github.com/yuin/goldmark/ast"
	"github.com/yuin/goldmark/extension"
	"github.com/yuin/goldmark/text"
)

func TestDebugAST2(t *testing.T) {
	src := []byte("- [ ] todo item")
	md := goldmark.New(goldmark.WithExtensions(extension.Strikethrough, extension.TaskList))
	reader := text.NewReader(src)
	tree := md.Parser().Parse(reader)

	ast.Walk(tree, func(n ast.Node, entering bool) (ast.WalkStatus, error) {
		if !entering {
			return ast.WalkContinue, nil
		}
		if n.Type() == ast.TypeInline {
			return ast.WalkContinue, nil
		}
		fmt.Printf("ENTER %T lines=%d\n", n, n.Lines().Len())
		if li, ok := n.(*ast.ListItem); ok {
			fmt.Printf("  ListItem Offset=%d\n", li.Offset)
			for child := li.FirstChild(); child != nil; child = child.NextSibling() {
				if child.Type() != ast.TypeInline {
					fmt.Printf("  child: %T lines=%d\n", child, child.Lines().Len())
					if child.Lines().Len() > 0 {
						for i := 0; i < child.Lines().Len(); i++ {
							s := child.Lines().At(i)
							fmt.Printf("    line[%d]: start=%d stop=%d text=%q\n", i, s.Start, s.Stop, string(s.Value(src)))
						}
					}
				}
			}
		}
		return ast.WalkContinue, nil
	})

	fmt.Println("\n--- ThematicBreak test ---")
	src2 := []byte("foo\n\n---")
	reader2 := text.NewReader(src2)
	tree2 := md.Parser().Parse(reader2)
	ast.Walk(tree2, func(n ast.Node, entering bool) (ast.WalkStatus, error) {
		if !entering || n.Type() == ast.TypeInline {
			return ast.WalkContinue, nil
		}
		fmt.Printf("ENTER %T lines=%d\n", n, n.Lines().Len())
		if _, ok := n.(*ast.ThematicBreak); ok {
			fmt.Printf("  ThematicBreak found, lines=%d\n", n.Lines().Len())
			for i := 0; i < n.Lines().Len(); i++ {
				s := n.Lines().At(i)
				fmt.Printf("    line[%d]: start=%d stop=%d text=%q\n", i, s.Start, s.Stop, string(s.Value(src2)))
			}
		}
		return ast.WalkContinue, nil
	})
}
