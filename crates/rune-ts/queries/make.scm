; Hand-authored offline against tree-sitter-make 1.1.1's own grammar.js and
; node-types.json, since the query the crate bundles speaks a legacy capture
; vocabulary. Captures map onto rune-syntax's CODE_SCOPES vocabulary by exact
; name or dotted prefix.

(comment) @comment

[
 "ifeq"
 "ifneq"
 "ifdef"
 "ifndef"
 "else"
 "endif"
] @keyword.control.conditional

[
 "include"
 "sinclude"
 "-include"
 "define"
 "endef"
 "export"
 "unexport"
 "override"
 "undefine"
 "private"
 "vpath"
] @keyword

(automatic_variable) @variable.builtin

(variable_reference (word) @variable)

(substitution_reference text: (word) @variable)

(variable_assignment name: (word) @variable)

(shell_assignment name: (word) @variable)

(define_directive name: (word) @variable)

(undefine_directive variable: (word) @variable)

(function_call function: _ @function.builtin)

(shell_function function: "shell" @function.builtin)

[
 "="
 ":="
 "::="
 "?="
 "+="
 "!="
 ":"
 "::"
 "&:"
 "|"
 ";"
] @operator

(string) @string

(raw_text) @string

(escape) @string.escape

(recipe_line ["@" "-" "+"] @punctuation.special)

(targets (word) @function)

((targets (word) @constant.builtin)
 (#match? @constant.builtin "^\\."))
