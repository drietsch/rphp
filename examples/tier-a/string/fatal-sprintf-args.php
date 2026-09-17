<?php
// Too few arguments: the count is taken over the whole format and reported
// once, counting the format itself (ArgumentCountError, exit 255).
echo sprintf("%s ok\n", "a");
echo sprintf("%s %s %s", "a");
