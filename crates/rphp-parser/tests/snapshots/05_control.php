<?php
if ($a) echo 1; else if ($b) echo 2; else echo 3;
if ($a) { } elseif ($b) { } else { }
if ($a): echo 1; elseif ($b): echo 2; else: echo 3; endif;
while ($a) echo 1;
while ($a): echo 1; endwhile;
do echo 1; while ($a);
for ($i = 0, $j = 1; $i < 2, $j < 3; $i++, $j++) {}
for (;;): break; endfor;
foreach ($a as $v) {}
foreach ($a as $k => &$v): endforeach;
foreach ($a as [$x, $y]) {}
foreach ($a as $k => [$x]) {}
switch ($a) { case 1: case 2: echo 1; break; default: echo 2; }
switch ($a): case 1: break; endswitch;
while (1) { while (1) { break 2; continue 1; } }
goto end; end: echo 1;
try { } catch (\Exception $e) { } catch (A\B|C) { } finally { }
declare(ticks=1) { echo 1; }
declare(ticks=1): echo 1; enddeclare;
declare(ticks=1);
{ echo 1; }
;
return;
