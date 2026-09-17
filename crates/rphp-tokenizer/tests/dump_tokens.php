<?php
// Oracle side of the differential test (see tests/differential.rs).
//
// Reads file paths from stdin, one per line, and prints for each file a line
// "== <path>" followed by one line per token: "<id>\t<line>\t<len>".
// Tokens come from PhpToken::tokenize(), which runs the very same
// ext/tokenizer loop as token_get_all() (same ids, same text, same line
// accounting) but also reports the line of single-character tokens, so the
// comparison covers lines everywhere. Single-character tokens have their byte
// value as id; the two-byte b"/B" opener has id 34 ('"').
//
// Run as:  php -n -d short_open_tag=0|1 -d error_reporting=0 dump_tokens.php < paths
// (error_reporting=0 keeps compile warnings such as "Octal escape sequence
// overflow" off stdout; they do not affect the token stream.)
while (($path = fgets(STDIN)) !== false) {
    $path = rtrim($path, "\r\n");
    if ($path === '') {
        continue;
    }
    echo "== $path\n";
    $src = @file_get_contents($path);
    if ($src === false) {
        echo "!! unreadable\n";
        continue;
    }
    $out = '';
    foreach (PhpToken::tokenize($src) as $t) {
        $out .= $t->id . "\t" . $t->line . "\t" . strlen($t->text) . "\n";
    }
    echo $out;
}
