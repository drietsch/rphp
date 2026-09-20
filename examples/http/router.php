<?php
if (preg_match('#^/static#', $_SERVER['REQUEST_URI'])) return false;
echo "router ", $_SERVER['REQUEST_URI'], " ", $_SERVER['SCRIPT_NAME'], " ", $_SERVER['SCRIPT_FILENAME'], "\n";
