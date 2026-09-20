<?php

// ext/tokenizer over the workspace scanner: the array shape of
// token_get_all(), token_name(), the T_* constants and PhpToken.
var_dump(extension_loaded('tokenizer'), TOKEN_PARSE, T_STRING, T_OPEN_TAG, token_name(262), token_name(59), token_name(9999), token_name(T_NAME_QUALIFIED));
$src = "<?php\nnamespace App;\n/** doc */\nfinal class Foo extends Bar { public function f(int \$x = 1): ?string { return \"a{\$x}b\"; } }\n?>\ntrailing";
$t = token_get_all($src);
var_dump(count($t));
foreach ($t as $tok) { echo is_array($tok) ? token_name($tok[0]) . ' ' . json_encode($tok[1]) . ' L' . $tok[2] : json_encode($tok), "\n"; }
$p = PhpToken::tokenize($src);
var_dump(count($p), $p[3], (string) $p[3], $p[3]->getTokenName(), $p[3]->is(T_NAMESPACE), $p[3]->is('namespace'), $p[3]->is([T_STRING, T_NAMESPACE]), $p[1]->isIgnorable(), $p[1]->getTokenName());
foreach ($p as $tok) { if ($tok->id < 256) { var_dump($tok->getTokenName(), $tok->pos); break; } }
var_dump(new PhpToken(T_STRING, 'x'), new PhpToken(59, ';', 3, 10));
var_dump(token_get_all(''), token_get_all('no php here'), token_get_all('<?php'));
