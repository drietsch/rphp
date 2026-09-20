<?php
// highlight_file()/highlight_string(): php's token-class colouring in the
// 8.3+ <pre><code> form, over the same scanner token_get_all() uses.
echo highlight_file(__DIR__ . '/highlight-source.inc', true), "\n";
highlight_string("<?php echo 1; ?>x");
echo "|\n";
var_dump(highlight_string("<?php\n\t\$x = 1; // 1\n", true));
var_dump(@highlight_file(__DIR__ . '/nope.inc'));
var_dump(show_source(__DIR__ . '/highlight-source.inc', true) === highlight_file(__DIR__ . '/highlight-source.inc', true));
ini_set('highlight.keyword', '#123456');
var_dump(highlight_string('<?php if (1) {}', true));
