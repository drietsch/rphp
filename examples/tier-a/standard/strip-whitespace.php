<?php
file_put_contents('strip-src.php', <<<'PHP'
<?php
// A comment
/* A block
   comment */
/** A doc comment */
function   foo( $a ,  $b )   {
    return $a    +   $b; # hash comment
}

$s = "a    string   stays";
$h = <<<EOT
  heredoc body
    keeps   its spacing
  EOT;
$n = <<<'NOW'
nowdoc
NOW
;
echo foo(1, 2), $s, $h, $n;
?>
Inline   HTML    stays
<?php   echo   1 ;
PHP);
var_dump(php_strip_whitespace('strip-src.php'));
file_put_contents('strip-src.php', "no php here\n  at all");
var_dump(php_strip_whitespace('strip-src.php'));
file_put_contents('strip-src.php', "<?=   \$x   ?>\n<?php\n\n\n\$y = 1;   // end");
var_dump(php_strip_whitespace('strip-src.php'));
file_put_contents('strip-src.php', "");
var_dump(php_strip_whitespace('strip-src.php'));
unlink('strip-src.php');
var_dump(php_strip_whitespace('nope.php'));
try { php_strip_whitespace(''); } catch (\ValueError $e) { echo $e->getMessage(), "\n"; }
try { php_strip_whitespace("a\0b"); } catch (\ValueError $e) { echo $e->getMessage(), "\n"; }
