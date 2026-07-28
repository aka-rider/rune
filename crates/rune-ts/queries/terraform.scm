; Hand-authored offline against tree-sitter-hcl 1.1.0's own grammar.js,
; since that crate exports no highlights query of its own. Captures map
; onto rune-syntax's CODE_SCOPES vocabulary by exact name or dotted prefix.

(block . (identifier) @keyword)

(attribute . (identifier) @property)

(variable_expr (identifier) @variable)

(function_call . (identifier) @function)

(get_attr (identifier) @property)

(string_lit) @string

(numeric_lit) @number

(bool_lit) @boolean

(null_lit) @constant.builtin

(comment) @comment

["for" "in" "if" "else" "endfor" "endif"] @keyword

["=" "==" "!=" "<" "<=" ">" ">=" "&&" "||" "!" "+" "-" "*" "/" "%" "=>" "?" ":" "<<" "<<-"] @operator

["{" "}" "(" ")" "[" "]"] @punctuation.bracket

["," "."] @punctuation.delimiter

(ellipsis) @punctuation.delimiter
