@echo off
rem =====================================================================
rem  One-shot commit script template.
rem  For AI agents: after finishing a change set, copy this file to the
rem  REPO ROOT as NNN-<short-english-slug>-gitcommit.bat where NNN is
rem  the next 3-digit number (001, 002, ...), replace __COMMIT_MSG__
rem  below with the real commit message, replace __ADD_PATHS__ with the
rem  space-separated paths this phase changed (use -A ONLY when no other
rem  uncommitted changes exist), and tell the user to double click it.
rem  Do not run the script yourself.
rem  The commit message must be a single line without %% ! ^& ^| ^< ^> ^^
rem =====================================================================
setlocal
set "MSG=__COMMIT_MSG__"

title %~nx0
cd /d "%~dp0" || goto :fail

echo === [1/4] git add ===
git add __ADD_PATHS__ || goto :fail

echo === [2/4] git commit ===
git config user.email >nul 2>&1
if errorlevel 1 (
    echo ERROR: git identity is not configured. Run once in any terminal:
    echo   git config --global user.name "Your Name"
    echo   git config --global user.email "you@example.com"
    goto :fail
)
git diff --cached --quiet
if errorlevel 1 goto :do_commit
echo Nothing staged. Changes were probably committed before. Skipping commit.
goto :do_push

:do_commit
git commit -m "%MSG%" || goto :fail

:do_push
echo === [3/4] git push -u origin HEAD ===
git push -u origin HEAD || goto :fail

echo === [4/4] Success. This script will now delete itself. ===
timeout /t 2 /nobreak >nul 2>&1
(goto) 2>nul & del "%~f0"

:fail
echo.
echo COMMIT NOT COMPLETED. This script kept itself so you can fix the
echo problem and double click it again. Steps already finished, such as
echo a commit that succeeded, are skipped automatically on the next run.
pause
exit /b 1
