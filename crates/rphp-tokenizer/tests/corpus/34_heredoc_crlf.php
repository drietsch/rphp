<?php
$a = <<<EOT
plain
EOT;
$b = <<<EOT
  with $var and {$obj->prop}
  and more
  EOT;
$c = <<<'EOT'
  nowdoc
  EOT;
$d = <<<EOT
EOT;
$e = <<<EOT

EOT;
$f = <<<EOT
x
EOT
;
$g = "dq
string $x
here";
$h = 'sq
string';
$i = 1; // comment
$j = 2; # comment
/* block
comment */
$k = <<<EOT
	tabs
	EOT;
?>
html
<?php
$l = 1;
