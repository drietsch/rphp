<?php
$a = "line1
line2\
line3\n
line4";
$b = 1;
$c = "x\u{zz}
skipped
lines";
$d = 1;
$e = "a
b $x
c\u{zz}
d";
$f = 1;
$g = "a
b
c";
$h = 1;
$i = `a
b $x
c\u{zz}
d`;
$j = 1;
$k = <<<EOT
a\u{zz}
b
EOT;
$l = 1;
$m = "\u{41}
$x\u{zz}

";
$n = 1;
$o = "$x\u{zz}

$y
";
$p = 1;
$q = "\u{zz}

";
$r = 1;
$s = "\
\\
\\
";
$t = 1;
$u = "\x0
\1
\u
\u{
\u{4
";
$v = 1;
$w = "$x
y
";
$z = 1; $aa = "$x
$y
$z";
$bb = 1;
