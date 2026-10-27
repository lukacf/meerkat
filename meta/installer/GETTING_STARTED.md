# Your first Meta Forge setup

Describe a useful job, shape its agent team, then install and try it on your own
Mac or Linux computer. Forge guides design and packaging on top of Meerkat
Core and Meta. See the [component overview](README.md)
for how they fit together. Forge is a developer alpha. A running process or an
attractive simulation does not establish that your job works.

You can design, save and build a setup now. Follow the installation controls
offered by your copy of Forge. **Install application** leaves it stopped for
private application setup before first use; **Install and start** also starts
an ordinary supported package. If **Setup pending** appears, keep the same folder
and saved settings and follow the [application setup guidance](#4-install-on-this-computer).

## Quick path

1. **[Open Forge](#1-open-meta-forge).** Unpack the complete prepared ZIP, run `bash start.sh`,
   and open the printed address. Keep that terminal open.
2. **[Describe one useful job](#2-explain-the-job-before-choosing-tools).**
   Choose **In my own words** to write, or **Answer two questions** to pick
   starting context. Then deliberately send your idea or starting point.
   Answer the guide's questions and watch the
   proposal take shape.
   When the proposal fits, choose **Use this plan**.
3. **[Choose what to build](#3-choose-a-team-or-a-custom-application).**
   Choose **Install agent team** when offered for agents, tools and Console. For custom
   behavior or screens, open **Try, build and run**, then **Build application**
   when its checks are ready. Optional trials let you inspect the proposed work.
4. **[Install](#4-install-on-this-computer).** Choose **Install on this computer**
   when offered for the checked application, or continue the agent-team setup. Pick an empty
   folder and enter private settings in the forms. Follow the offered installation
   action. **Install application** leaves the application stopped for setup;
   **Install and start** also starts an ordinary supported package.
   For Google Cloud, or when Forge offers a remote Linux destination, follow the
   [remote setup steps](#install-a-custom-application-remotely) instead.
5. **[Use it](#5-complete-one-useful-job).** Open the installed application's
   current interface link, send a small real request, and inspect the result.
   Correct a fact and ask a follow-up in the same conversation.
6. **[Return to your work](#6-return-without-starting-over).** Reopen **Your
   setups**, select this setup, and use its saved installation controls. Start
   the same installation when needed and check that its history is still there.

The sections below explain these steps, provider setup and recovery. Keep
passwords and API keys in the private forms throughout the journey.

## 1. Open Meta Forge

For the Meta 0.9 preview, use the complete prepared product ZIP and its
generated README. Unpack it into a folder you will keep. Open a terminal there
and run:

```bash
bash start.sh
```

You need a complete Python 3.11+ installation with virtual environment and pip
bootstrapping support (`venv` and `ensurepip`), plus internet for initial downloads.
Use the operating system and hardware named in your bundle's README. Selected
product bundles include their application runtime and select it when you run
`start.sh`; they do not need application-host environment variables.
The standalone wizard opens without Node.js or npm. Follow the printed address, normally
[http://127.0.0.1:8787](http://127.0.0.1:8787), and keep the terminal open.
A source-only Meta 0.9 checkout does not include the selected product runtime;
its launcher stops before dependency preparation. Maintainers must first assemble
[the complete product package](README.md#maintainer-preparation). Installing
Node.js or a Python SDK alone does not turn that checkout into a launchable ZIP.
See [startup prerequisites](README.md#open-the-wizard) if preparation fails.

To install applications, run Forge on CPython 3.11: the Toolkit packages in this
release are built for it. On Ubuntu 24.04, whose `python3` is 3.12, run
`uv python install 3.11` (or install `python3.11` and `python3.11-venv` from the
deadsnakes PPA). If Forge already ran in this folder, stop it and move only
`.venv` aside, as [the README](README.md#open-the-wizard) shows; saved setups are
kept. Then start Forge with `PYTHON_BIN` set to that interpreter, for example
`PYTHON_BIN="$(uv python find 3.11)" bash start.sh`.

If you see **Forge dependency preparation did not finish**, correct the error
shown above it and use the printed retry command in the same folder. A download
failure may need restored internet access. Keep `.venv`, your saved setups and
settings; dependency preparation may be partial, and the web server has not
started. The command retains your startup settings and `--prepare-only` if used.

If Forge is already running, reopen the address printed in its terminal.
If port 8787 belongs to an unrelated application, use
`META_FORGE_PORT=8788 bash start.sh` for another local port. Changing the port
changes the address, not the saved server-state folder. Unsent browser drafts
remain at the previous address. It does not recover an existing running Forge
or give it separate saved state.
`META_FORGE_HOME` can select a private state folder; keep using that
same folder when you return. `MOBKIT_LAUNCH_PORT` and `MOBKIT_LAUNCH_HOME` remain
compatibility aliases. If both names are set, the corresponding `META_FORGE_*`
setting takes precedence.

At **What would you like to get done?**, describe one job, project or recurring task
in the idea box. Forge helps you create your own team of AI agents around that
need. A rough sentence is enough. Above the composer, two starting routes stay
visible: **Write - In my own words** and **Choose - Answer two questions**.
Switch between them without losing your writing. The short notice below the
composer explains that nothing is sent until you choose Send. Choose
**Send to guide**. On desktop,
**Enter** sends and **Shift + Enter** adds a new line. On a phone, Return adds a
new line; use the Send button to submit. If a model provider is unavailable, the action
says **Connect your guide** and opens provider setup without sending your idea.

Not sure what to write? Choose **Answer two questions** and answer two quick
questions: who it is for, then what kind of help would be useful. Forge asks one
question at a time, showing **Question 1 of 2** or **Question 2 of 2**. Each choice
has a short description to help you decide. If you have already written something,
a read-only excerpt marked **Your idea - not sent** stays visible during the
questions. Choose **In my own words** or **Back to my words** to return to editing.
The second choice adds a separate
**Starting point** summary marked **Not sent** above the composer. Your own words
stay exactly as written. Add a task using the
prompt above the box, or send just the starting point and let your guide help
you find one. Selecting either answer sends nothing. Choose **Send to guide**
to submit your starting point, with or without a written task. The **For example**
placeholder in the empty task box is inspiration only; it is never added to your
writing.

Select either underlined answer in the starting point to reopen that question.
**Cancel change** keeps both existing answers unchanged. Use **Remove** to discard
the starting point while keeping your writing. Inside
the helper, choose **Back to my words** if you have a draft, or **Write it myself
instead** if the draft is empty, to close the questions without adding a new
starting point. Your selected choices and writing remain separate until you deliberately
send them to the guide.

Optional examples under **See example ideas** fill an editable draft and do not
send it. If you have already written something, an example opens as inspiration
without replacing your text. **Add inspiration to my idea** appends the example
after your latest writing, and **Keep my idea** dismisses the preview without
changing it. The circular opening illustration is a preview, not your proposed
team. Your actual proposal appears as the guide designs your system. Use
**Pause motion** to pause the illustration. On a phone, choose **View** on the
miniature illustration to open the larger view.

If an attached file keeps uploading, choose **Stop waiting**. Your draft and
pending files stay on this page. The upload is unconfirmed, so choose **Retry
upload** deliberately or **Remove** the files to continue without them. Neither
action sends your message.

If Forge shows **Conversation creation is not confirmed** or **Message delivery
is not confirmed**, choose **Check saved conversation**. Your writing and any
uploaded files stay available while Forge checks what was saved. Checking does
not send the message again. If nothing is confirmed yet, keep your draft and
check again. Before a first conversation is confirmed, **Open a new draft** also
lets you start again with your current writing. It opens an unsent draft; it does
not cancel the original request, which may still finish. The old creation check
is retired when you open that draft. Choose **Send to guide** deliberately when
ready. This shortcut is not offered for uncertain delivery in an existing
conversation.

When a new conversation is recovered, your latest writing returns to its idea
box. Choose **Send to guide** when you want to send it. If you were attaching a
file, select the file again deliberately; recovery does not resume that upload.
For a message already received by the guide, Forge resumes its saved progress.
After your first idea is confirmed as saved, it appears in the conversation and
the input clears. You can write your next message while the guide works;
**Waiting for guide** stays disabled until the reply arrives. Your next draft
stays exactly as written and is never sent automatically. A failed or interrupted
response keeps its saved input available for the offered retry. If you are
looking at the proposal preview, choose **Return to conversation** when offered
to find those recovery actions. Build and installation actions wait while
message delivery is uncertain.

On a phone, **New response** appears in **Your team** when a saved guide reply
arrives, including before the first proposal exists. Choose it to return to the
reply and its current choices. Your next draft stays in the composer. Completing
the starting questions reveals your unsent starting point. With no written draft,
focus moves to **Send to guide** instead of opening the phone keyboard, unless
you have already moved to another control.

If a model provider is needed, Forge opens its settings and keeps your idea and
starting point. The provider screen shows them separately, both marked **Not
sent**. Connecting, saving a key and starting the guide send neither.
Select your provider, enter its API key, and choose
**Save locally**, then **Start guide**, or use the existing configured provider.
This powers the guide, builder and animation agent. Installed applications have
their own private setup. Enter keys in these forms, not in the conversation.
You can also open these settings through **Guide settings** at any time.

**Guide console** opens Forge's own setup agents in a new tab. Your proposed
team stays in **Your team**. Use the installation's interface links after
installing it to talk to your own agents.

Need an API key? Create one through [OpenAI's key dashboard](https://platform.openai.com/api-keys),
[Claude's key setup guide](https://platform.claude.com/docs/en/get-api-key), or
[Gemini's key setup guide](https://ai.google.dev/gemini-api/docs/api-key), then
return to Forge. The form's **I do not have a key yet** disclosure explains
these steps. Keys are saved privately on the computer running Forge, including
when you open Forge from a different device. They are excluded from the setup
conversation and deployment downloads.

After **Save locally**, the form keeps its confirmation and **Start guide**
action visible. Choose **Start guide**, then wait for **Ready for your first
message** and choose **Back to my draft**. If you selected help without typing, choose
**Back to my starting point**; with neither, choose **Return to Forge**. Review the
retained draft and choose **Send to guide** to send it. Saving the key
and starting the team do not test model access; your first message does.

If a guide message reports that your locally saved API key was rejected, wait
for current work to finish. Open **Guide settings**, choose **Stop setup team**
and wait for it to confirm the stop. Enter a replacement key for the same
provider, choose **Save locally**, then **Start guide**. Saved conversations are
retained; this does not resend the failed answer. Return to the conversation and
use its offered retry deliberately. Keys configured by the server environment
cannot be replaced in this form.

While the guide starts, its **Now** panel explains the current preparation
phase. The first start may download its runtime. A confirmed failure shows
specific guidance, including storage trouble on the computer running Forge.
Use **Check guide status** to read the current state without repeating Start.
Your draft and starting point remain unsent throughout this step.

If saving cannot be confirmed, choose **Check saved settings**. Forge clears the
key field and keeps your unsent idea. The check reads the current configuration;
it does not confirm that the key you just entered was saved. Nothing is resent
or started automatically. After the check, you can use the saved configuration
or deliberately enter and save a replacement key.

## 2. Explain the job before choosing tools

Describe the useful result, who it is for, how often it should work, how people
will talk to it, and what needs approval. Agent names and connectors can come later.

Select the answer cards that fit. Your selected answer appears above the composer;
you can remove a choice there without losing your writing. Add optional context
in the same composer, then choose **Send answer**. Single, multiple and combined
choices use this same Send action. Selecting or removing a choice sends nothing.
Use **More than one fits** when offered to combine choices. The guide explores
audience, scale, deployment, reliability and interfaces before narrower connection
details.

After your first message, the conversation saves automatically in **Your setups**
as you go. Selecting choices or editing text does not send it. Unsent text and
choices are browser drafts, separate from saved conversation history. If Forge
warns that a draft is page-only, keep a copy before closing or reloading it.
You do not need **Save revision** to save the guided conversation.

Your first description starts the design. Watch the proposal and its animation
take shape as your guide works, inspect responsibilities and collaborators, and
correct the design with the guide. You can also ask for a clearer illustration,
for example, "Keep the circles, make specialist labels larger, and put the
explanation in the caption." The guide keeps those visual preferences separate
from changes to the agent team. Ask it to clear a preference when it no longer
fits. **Refine animation** remains available for a specific visual revision;
its draft is sent only when you choose to submit it.

While an updated illustration is being prepared, Forge keeps the previous scene
visible. When offered, **Changes being illustrated** or **Changes awaiting
illustration** lists the saved agent-class changes that are not yet shown,
including proposed starting-agent counts. Expand the remaining rows to read
more. These rows describe changes to the proposal; they do not confirm that a
new illustration or installed team is ready.

When the design fits and necessary decisions are
resolved, choose
**Use this plan**. Remaining implementation or connection work stays visible.

This saves your design and its choices. If Forge shows **Your design is saved;
package preparation is pending** or **Accepted design - package preparation
pending**, read the displayed reason to find the next step.

For a cloud team, you can refine the design before choosing a Google Cloud
project. Before preparing its package, tell the guide your project ID or enter it
in the advanced editor, then review and accept the updated plan. This supplies the
destination; it does not create a VM or establish that the deployment is ready.

If a selected connection setup helper or custom native tool bundle cannot be
packaged with the current installer yet, keep the full saved design for later,
or choose
**Revise with guide** to discuss alternatives while preserving your requirements.
Any deliberate reduction in scope must be an explicit revised proposal that you
review before accepting. Enabling an optional Toolkit catalog or setup controller
does not remove this package limitation. New team packages, fresh application
builds and runtime rebuilds wait for that support.
Your existing installation remains available through its own controls.
The header does not offer **Build** for this pending state unless saved application
work takes precedence; the revision action remains available.

If acceptance is waiting or cannot be confirmed, choose **Check saved plan**.
This reads your saved conversation without accepting the plan again. Your unsent
writing stays editable. If the saved version has not changed, Forge keeps the
plan unconfirmed. After checking the same reviewed plan, you can choose
**Try saving this plan again**. This deliberately saves that plan; it does not
build or start an application. When the saved conversation has advanced, Forge
shows that current version and restores its available actions.

## 3. Choose a team or a custom application

For configured agents, tools and native Console, choose **Install agent team**
when offered. This does not create custom screens or application-specific behavior.
An installed Forge (the packaged product) does not offer agent teams: they use
the older MobKit 0.8 runtime. There, choose **Build application**, which installs
your agents as an application on Meta 0.9.4.

For custom behavior or UI, open **Try, build and run** and find the
**Custom application** section.
Choose **Prepare app checks** if prompted, wait for readiness, then choose
**Build application**. The builder writes code and tests; an independent reviewer
checks it. If unfinished work remains, choose **Continue repairing application**
when offered. After a failure or interruption, **Resume saved build** continues
the saved work when available. Some saved builds can continue from their exact
retained source even while new package preparation is pending; wait for the
current build status before using that action. You can add a note about what
needs to change. Linux authoring has additional
[isolation prerequisites](README.md#standalone-bundle).

If **Runtime and host updates** is offered, **Rebuild with updated runtime**
creates another checked package using the runtime and generated host available in
this installer while keeping your application work. It does not update installed
files. Review the new package, then use the saved installation's update controls
when available. This action does not select the pending Meta/Toolkit composition
or remove a displayed package-preparation limitation.

After a failed or interrupted build, **Start a fresh build** may also appear
under **Runtime and host updates** once Forge has verified the saved source and
confirmed cleanup. It starts again from your accepted setup with the current
runtime, leaving the unfinished application code and its checks in the previous
build. The old source and history stay saved, and your unsent builder note stays
in the note box. Use **Resume saved build** to continue that saved application
work instead, provided Forge still selects that build's original runtime.
If that selection has changed, resuming is refused. A fresh build still needs
the displayed package and app-check
prerequisites; it leaves your installed application unchanged.

If **Stopping builder...** or **Closing build runtime...** appears, wait for the
existing build runtime to finish closing. If offered, **Retry cleanup** retries
that cleanup. **Cleanup unavailable** means Forge cannot confirm cleanup yet;
your saved source and reports remain available. Another build waits until
cleanup is confirmed.

If **Preparing...** stays visible, choose **Check preparation status**. Forge
reads the existing preparation status without starting another download.
Continue when **App checks ready** appears.

If **Starting builder...** stays visible while Forge offers **Show saved build**,
choose that action to continue from the newer saved result. Your builder note
stays in place. This does not start another build.

If **Checking your saved build** appears, wait for the status check. If **Build
status is unavailable** appears, choose **Review build status**, then **Refresh
build status**. Your note and saved source are kept; refreshing does not start
another build.

If **Your saved build needs attention** appears, Forge found source without a
usable build selection. **Find your saved build** checks that saved work. Choose
**Restore saved build** for the original build you want to inspect. This restores
only its selection; it does not start a builder, replace source, or confirm that
an earlier worker stopped. If worker cleanup is unconfirmed, another build stays
unavailable. Incomplete saved work is preserved and cannot be selected from a
folder name alone. If a restore response has not returned after 30 seconds,
Forge releases the wait and checks build status. The original selection save
may still finish; Forge does not send it again automatically. After an
unconfirmed response, refresh build status or choose **Check saved work again**
before choosing another action.

Keep these stages distinct:

| Stage | What it tells you |
| --- | --- |
| Proposal animation | An illustration of the current design. It does not execute the agents. |
| **Agent design** trial | Real native collaboration with sample data, without live services or application adapters. |
| Checked application build | Code passed the build/review path. Destination behavior still needs testing. |
| **Built application** trial | Actual application callbacks run with isolated sample data, where supported. It does not install the app or verify live integrations. |
| Installation | Checks startup and runtime availability. A useful job remains to be demonstrated. |

Optional trials live in **Try your mob**. Choose **Agent design** or **Built
application**, enter a realistic request or select a sample, then choose **Try
agent design** or **Try application request** to start it. Inspect its transcript
and result. **End trial** closes a continuing trial. Sample state is separate
from your installation.

For an optional team download, choose **Download agent team package** when offered.
For a checked custom application, open **Manual exports and terminal setup** and
choose **Download checked application** after its build is accepted. If
**Download native setup only** is offered, it contains the team, runtime host and
configuration without the builder's custom application code. A stalled download
releases the guide after 60 seconds and shows an error. Your
draft stays editable; choose the download again when you want to retry. This
reads the checked package and does not start another build or installation.

After a local application has been installed, its **Run** area also offers
**Keep a copy of this installed version**. Choose **Download installed package**
to recover its original checked source package, including when a newer build
needs repair. Forge checks the saved package against that installation's receipt.
The download excludes private settings and saved application records, so keep
those separately if you are backing up your application. It does not select an
older build, rebuild the application or change the installation.

## 4. Install on this computer

For a checked custom application, choose **Run**, then **Install on this computer**
when offered.
For an agent team, **Install agent team** checks the selected destination. If the
plan matches this computer, it opens private setup. If a Linux plan is being
prepared on a Mac, open **Install this agent team on your Linux computer**, choose
**Download agent team package**, and follow the listed steps on that destination.
A Mac plan prepared on Linux offers its Mac package and manual steps. Keep the
selected destination; you do not need a custom application build for this path.

If Forge says **This private runtime candidate is admitted only through the checked
update of an existing stopped installation**, this package cannot create a new
installation, even when it matches this computer. Keep the existing project
folder and use its offered **Stage accepted update** or recovery controls. Do not
extract the candidate into a new folder to work around the restriction. Ordinary
supported packages still offer **Install on this computer** or **Install into
another folder**.

For a fresh installation, choose a new empty folder, such as `~/src/my-system`.
Supply the requested private
settings and sign-in details. Using Forge's existing provider keys is an explicit
choice. Complete any missing connection prerequisites.

If setup asks for **Console user**, enter the local identity for this
installation. If the application's setup instructions name an operator, use
that exact identity. This is separate from your model provider login; entering
an identity does not grant it administrator access.

If setup asks for a Console password, choose one and keep it somewhere you can
find when you return. Your provider API key powers the agents; your Console
password opens this installation's private Console. Use the Console password
on its sign-in page, not your API key or your provider website password. The
sign-in form asks only for that password; the installation applies your configured
user identity automatically.

Follow the installation action and its checks. **Install and start** starts an
ordinary supported package. **Install application** checks the selected
Meta/Toolkit runtime and leaves the application stopped. Its **Application setup
is next** card means there are no running agents or ready interface links yet.
Saving private settings keeps it stopped; ordinary Start cannot replace that
setup. Choose **Review private settings** if required settings are missing. When
**Prepare application setup** is offered, choose it first and wait for confirmed
preparation. It retains this installation's private setup inputs and keeps the
application stopped. Then, when available, **Set up and start application**
completes setup and starts the agents;
**Start installed application** returns to a completed setup after a clean stop.
**Check setup status** reads the existing state without sending setup again.
After a completed setup and a clean stop, **Run in background** can be offered
as a separate choice when this installation supports it. Read the service's
actual persistence description; it does not always run after logout or reboot.
If the reply is lost, check the same installation's status before sending
another command. This completed-setup background control is reviewed but not
yet enabled in the served alpha; real service operation remains unproved.
If this copy of Forge lacks an application setup runtime, it cannot finish that
setup. A selected product bundle supplies its runtime through the normal
`start.sh`; an unconfigured source checkout does not. If **Setup pending** remains,
keep the same installation folder and saved settings. **Check setup status** only
reads availability; connecting your guide or saving private settings does not
supply a missing setup runtime. Ordinary supported packages retain **Install and start**.

Once the installation is ready, use its current links and sign-in details.
Prefer the application's everyday UI when supplied; an agent team offers its
named conversation owner in native Console.

If **Checking your installation status** appears, wait for the current check.
If **Installation status is unavailable** appears, choose **Review installation
status**, then **Check installation status**. The saved receipt and drafts stay
visible while Forge checks again. Wait for the checked result before using the
installation commands; the saved receipt describes the earlier observation.

If a running application's UI link is missing, choose **Retry UI links** and
wait for the link checks. Your edited task and problem-report drafts stay here.

For an application whose first-use details cannot be checked, keep editing or
copying your drafts while you retry **Check first-use details**. For an agent team
whose conversation owner or Console cannot be checked, choose **Recheck agent
team details**. If the page says storage is unavailable, keep a copy before
closing or reloading it.

A stalled UI-link or first-use details read releases its retry control after
30 seconds. Retrying these checks reads the installation's details; it does not
submit your task or start another installation. A timeout alone does not tell
you whether an installation operation succeeded or failed.

If Forge says application links, Console links or installation commands are
paused, choose **Check installation status** to recheck. Installation details
can remain visible while the saved command is unconfirmed. You can still edit
and copy drafts; recording success,
requesting improvements and installation commands wait until tracking can be
checked. Keep a copy of your drafts before closing if browser storage remains
unavailable.

If Forge confirms the saved command and current installation while its original
response is still waiting, choose **Return to checked installation** when
offered. This restores the checked controls without sending Start or Stop again.

If an installation command reports an error, some of its work may already have
finished. Wait for the checked installation status before choosing another
action. For example, an application can be confirmed stopped while its Stop
request is recorded as failed. The stopped state does not prove that setup or
recovery completed. Keep the same installation and use its offered
controls; do not send another Stop just to clear the error.

The same option can appear after Forge has checked that a new installation was
admitted, even while its original response is waiting. It uses the checked
installation status without submitting another installation or cancelling the
first one. Admission alone does not mean the application has finished starting.

Interactive supervision is the default: keep Forge's terminal/server running.
Closing its browser page leaves the application running; stopping Forge
interrupts its supervised runtime. Explicit background services have different
[lifecycle limits](README.md#install-on-this-computer).
Cloud, knowledge services and connectors need separate setup and verification;
complete real GCP deployment and automatic Toolkit onboarding remain unproved here.

### Install a custom application remotely

These steps apply to ordinary supported application packages. Remote setup and
protected access for selected Meta/Toolkit applications are integrated in source
but are not enabled in the served alpha. If your plan needs that path, keep its
saved design until it is enabled and verified with the matching qualified
runtime. A complete real cloud journey remains unproved.

For Google Cloud, install the official [Google Cloud CLI](https://docs.cloud.google.com/sdk/docs/install-sdk)
on the computer running Forge and [sign in through that CLI](https://docs.cloud.google.com/sdk/docs/authenticate)
before checking cloud access. If Forge says the CLI was not
detected when it started, installing it does not update the running Forge process.
Finish existing work, stop Forge normally and wait for confirmed cleanup before
restarting from the same retained folder and state directory. If work ownership
or cleanup is unconfirmed, resolve that state before restarting. Reopen the same
saved setup, then check cloud access again.

For a checked custom application targeting Google Cloud, open **Run**, then
choose **Check cloud access**. Select an account already signed in through
Google Cloud CLI and choose your project. Choose **Check destination** and
resolve the displayed access and billing checks. If you choose a different
project from the accepted build, **Use this project** updates the proposal;
accept a fresh build before continuing. Review the listed resources, access
changes and ongoing VM and disk charges, then choose **Create this cloud
computer** when offered.

When the computer is prepared, choose **Continue to application setup**, then
**Continue to private application setup**. Choose **Continue to private setup**,
enter the package's private settings and choose **Install and start in Google
Cloud**. Computer preparation alone
does not mean the application is installed.

For an existing Linux destination, enter its actual hostname or address, SSH login and
project folder when offered, then choose **Connect and check destination**.
SSH access must already work without an interactive password, and the
computer's host key must already be trusted here. Forge does not read
`~/.ssh/config`, so its aliases and key settings do not apply. It uses the default
SSH identities or your SSH agent. For a custom key not available that way, ask that
computer's administrator to follow the [private SSH profile instructions](README.md#install-on-this-computer)
before starting Forge. Resolve the displayed Python,
user-service and selected connector or knowledge prerequisites before
**Continue to private setup** and **Install and start on Linux**. Review the
user-service continuity check before relying on unattended operation; running
now does not prove the application will survive logout or reboot.

Use the checked application link to try a real task. If native Console is your
interface, choose **Connect browser to native Console**, then **Open native
Console**, and keep Forge open. Use **Recheck original cloud setup**,
**Recheck remote status** or **Recheck browser connection** for the corresponding
uncertain step. Use **Stop owned application** and **Restart accepted
application** when offered. Closing browser access does not stop the remote
application. Stopping the application does not remove billable cloud resources;
use **Review cloud removal** to inspect that separate action.

Agent teams use their separate destination package and manual instructions.
These guided remote steps apply to checked custom applications. A complete
live cloud journey remains unverified in this alpha.

## 5. Complete one useful job

For a custom application installed on this computer, find **Use** followed by
your application's name. Open its checked application UI link, sign in if asked,
and try a job using the application's own controls. Preparing or copying a
request in Forge first is optional.

If the checked package has no custom UI links, use its available **Open native
Console** link and named conversation contact. If native chat is this
application's everyday interface, sign in if asked, choose that contact, type or
paste a request, review it, then send it there. Continue in the same conversation
for replies, tools and follow-ups. Follow the application's own instructions
when it uses another everyday interface.

Under **Prepare a request (optional)**, choose a starting idea to fill an empty
draft, or describe your own job. If you already have writing,
another idea opens as a preview. **Keep my draft** closes it without changing your
writing; **Use this idea** replaces the task deliberately. You can keep editing
while the preview is open. Neither action sends the job. **Help with this job**
explains a suggested job's inputs, prerequisites and where to find the result.
It appears beside the draft when there is room and below it in a narrow panel.
Check that its instructions still fit any edits you made.

If **Continue with this task** is offered, it opens your task as an editable draft
in the application. Otherwise, use **Copy task** and the application's usual
controls. Choosing an idea, copying or opening a draft does not send the job;
use the application's send and confirmation controls. While clipboard copy is
pending or unavailable, **Select task to copy** selects your current draft for
manual copying.

Use a small real request in the installed application's actual interface. Inspect
the answer and saved result, correct a fact, and ask a follow-up. Startup health,
sample trials and a promise to remember are not substitutes for a useful result.

For a custom application, choose **It worked** under **How did it go?** after
checking the result. This saves your report of the experience; Forge has not
independently verified it and the installation does not change. Once saved,
**Choose the next job** takes you to the starting ideas, or the task field if
there are no ideas. **Write your own next job** takes you to the editable task.
Your drafts stay unchanged until you edit them.

For a custom application, open **Report a problem or request an improvement** in
Forge's installed-use area, choose what happened, describe the change, and choose
**Improve this behavior**. This requests another checked build, leaving installed
files and records unchanged. The form asks for acknowledgement if it will improve
a newer candidate. Applying a compatible update is a separate deliberate action.

Once the improvement passes checks, choose **Review improvement**, return to
**Run**, and check the installed project folder. If **Stage accepted update** is
offered, use it to check compatibility. While **Forge is checking update
compatibility** is shown, nothing is being applied. **Check installation status**
reads the saved installation; wait for the compatibility check to return before
another installation action. Stop the application and wait for
confirmed cleanup when prompted, then choose **Apply and verify update** and
follow its checks. If the application stays stopped, choose **Start installed
application** when offered before opening its current links. If **Recover this
application** appears, use **Retry current application** or **Recover previous
application** only when offered, and wait for the checked recovery result before
ordinary Start. Keep the same project folder so Forge can use the recorded update
and recovery activity.
If you stop during a runtime update, Forge waits for any storage operation already
in progress before confirming cleanup. Keep the same folder and its native
reports; recovery preserves current records. Use **Retry current application**
only when offered to recover the exact accepted runtime.
**Preview update compatibility** checks compatibility only; it cannot apply an
update. These application updates are separate from the installation-support
updates described below.

After a Meta Toolkit application finishes its setup, Forge applies only updates
built from the same application source that keep its plan, definition, tool
policy, protected settings and runtime. **Stage accepted update** shows why any
other update is blocked; install such a build into a new empty folder instead.
**Apply and verify update** installs and checks the new source and leaves the
application stopped. **Start installed application** then restores its completed
setup with the new source. If that Start fails, choose **Restore previous
application** when offered. After Forge restarts, use **Continue setup** once
before you stage or apply an update.

An application without a setup helper takes the same kind of update: Forge
applies only updates built from its own application source that keep its plan,
definition, protected settings, native instructions and runtime. **Apply and
verify update** installs and checks the new source and leaves the application
stopped; **Start installed application** then runs it, and **Restore previous
application** returns to the previous source. After Forge restarts, use
**Continue setup** or **Start installed application** once before you apply an
update.

For an agent team, tell the setup guide what should change and review the revised
plan. This does not modify the installed team.

## 6. Return without starting over

If Forge is still running, reopen the address printed in its terminal. Only if
Forge has stopped, run the same startup command in the same retained Forge
folder. Keep the same private state directory: retain your `META_FORGE_HOME`
value, or your existing `MOBKIT_LAUNCH_HOME` alias. The script reuses its prepared
Python environment; keep both the wizard state and installed project folder.

A Forge whose guide runs on Meta 0.9.4 or later starts a fresh setup team in the
same state directory. Your saved setups, proposals, provider settings and
installed applications stay, and the previous setup team's state is kept
unchanged, but the new team does not recall earlier conversations. To carry them
over, follow [Work with the setup team](README.md#work-with-the-setup-team)
before the first start.

On the opening screen, **Pick up where you left off** may show a saved setup
once the list has loaded. Check its name and choose **Continue** to open that
conversation. To choose another saved setup, open **Your setups**, select it and
wait for selection to finish. Opening a saved conversation does not send a message
or start a build or installation.
If its list cannot be read, choose **Retry saved setups**. A retained list is
labelled as the last successful check until the refresh succeeds.

To review a connection from a Toolkit recovery file, open **Saved tools** in
your saved setup and choose **Choose recovery file**. Check the full request and
its audience before choosing **Review current settings**. **Discuss how this
fits my mob** sends a question to the guide and keeps your composer draft. This
review does not install or sign into the connection. If the panel is unavailable,
its host needs the [Saved tools configuration](README.md#review-saved-tool-settings).

The installation panel identifies the installed version even if the proposal has
changed. Find it through **Run**, or expand **Try, build and run**, and check the
project folder shown before using its controls.
For a stopped local background application, **Installation support** may offer
**Review update**. Review it, then choose **Apply support update** to update the
prepared background support while retaining the application, private settings and
agent history. The application stays stopped. Complete this before staging an
application update. If the connection is interrupted, Forge checks the saved
command; choose **Resume this update** only when offered to continue that same
update. The **Technical details** disclosure holds its saved identifiers.

Moving an installed application to Meta 0.9.4 changes its saved state for good.
Meta 0.9.4 keeps conversations and agent identities in Meerkat Core's own stores,
so the application's saved state must be migrated once, while it is stopped,
before 0.9.4 can start it. Back up the stopped installation first: that backup
is the only way back to the earlier release.
[Upgrading to Meta 0.9.4](https://docs.rkat.ai/meta/guides/upgrading-to-0-9-4)
describes the migration and lists the earlier releases it accepts; check that
your installation's release is listed before you update. Forge itself does not
run this migration: it keeps each installed application on its Meta release, and
an update that would cross to Meta 0.9.4 is staged as blocked and cannot be
applied, without changing anything.
Keep using the Forge copy that installed the application to start, stop and
update it.

If **Retry installation** is offered, use it to finish installing the saved checked
package in the same folder. Ordinary Start and private settings remain unavailable
until its files are verified. Otherwise, choose **Start installed application** or
**Start installed agent team** when offered for a stopped installation, then wait
for checks and use its current links. Another installation does not resume this
one's history.

Use the application's UI link for everyday work. For native chat, choose
**Open native Console** or **Open agent team Console**. The direct administration
link is also under **Runtime and native Console details (optional)**. Sign in
with the Console password you chose during private setup, then select the
conversation owner shown by Forge in the Console sidebar. You do not enter a
user identity again on the sign-in page. A loopback-only
Console may open without a separate sign-in. Signing in does not send a job.

**Sign out** in the password-protected Console ends your current Console login
without stopping the application or deleting its saved state. Successful sign-out
returns to the password screen. If that screen does not appear, treat sign-out
as unconfirmed; closing the tab is not confirmation.

To add another conversation channel to a custom application, return to the same
saved setup. Tell the guide which people should use it and whether it should
share the existing agents and records. For example: "Let me talk to this team
through Telegram as well as the Console, using the same saved collection." A
connection that lets agents read a service's data does not automatically provide
a place for people to chat.

If your installed Console offers **Connect setup**, use it to configure the
connections supported by that application. Return to the same Console to
continue setup, including when the app runs in the background. This requires an
app that already supports setup and a current authorized login; an older app
does not gain setup support just by being installed or running.

Some installations instead offer the optional **Set up your app** conversation.
Its setup component is selected by the installation operator. The two panels
have different runtime requirements; see
[Installed application setup](README.md#set-up-an-installed-applications-channels)
for the distinction.

Keep passwords and tokens in the protected forms, not in your messages. Closing
either panel leaves the application running. A proposed or saved setting is not
proof that a channel works; complete its setup, then send and receive a real
message through that channel.

If **Set up your app** reports missing native authentication, missing application
context or an unsupported setup runtime, return to the saved setup in Forge and
review that part of the application with the guide. Console sign-in alone does
not make that optional component compatible. Its **Check current status** action
only reads the original setup result; it does not restart the setup agent or
repair the application.

If the installed app cannot support the requested channel yet, refine its plan
with the guide and review the resulting build. Apply it to the same installation
only through the offered compatible-update controls described in
[Complete one useful job](#5-complete-one-useful-job). Keep the existing project
folder and records. A separately installed example has its own settings and
records; it does not extend this installation.

For an installation on this computer, recover a forgotten Console password by
stopping this installation in Forge and waiting for cleanup. Choose **Change
private settings**, enter a replacement Console password, then **Save private
settings**. Leave other configured fields blank to keep them. Start the same
installed application or team again and use its current Console link. These
controls keep its installed files and saved records.

For a guided remote Linux or Google Cloud installation, stop the same installed
application and wait for Forge to confirm it is stopped. If **Change private
settings** is offered, open it, enter a replacement Console password and choose
**Save private settings**. Saved private values stay hidden; blank fields keep
them, and optional fields have an explicit removal control. After Forge confirms
the save, start that same application explicitly and use its current Console
link. Saving keeps it stopped and preserves its installed files and saved records.

Older remote installations do not support this settings action. Forge identifies
that limitation after checking the installation. Arrange password recovery with
that computer's administrator while keeping its existing project folder and saved
records. Remote stop and restart reuse its existing private settings.

If Forge cannot confirm a settings save, the original request may still finish.
Your changes are kept in the open form. Checking status or reloading the
field list does not establish that those exact values were saved; Forge does not
send the save again automatically. Use the displayed recovery controls before
deciding whether to submit again. Saving a password is not proof of Console login
or native authentication.

If a start on this computer fails in either the default interactive mode or
background mode and you need to correct settings, choose **Stop and verify
cleanup** in that saved installation when offered. Wait until Forge confirms it
is stopped and cleanup is complete, then choose **Change private settings** when
offered. After saving, start the same installed application or team explicitly.
Cleanup does not restart it or resend a task. If the control is unavailable, keep
the same folder, review the displayed reason and use **Check installation status**
before continuing.

If Forge cannot open the browser's saved draft, choose **Try again**, or
**Download saved draft** to keep an exact copy. **Use a fresh draft instead**
offers a separate confirmation before replacement. Saved conversations and
installations are kept separately. If browser storage is unavailable, the page
identifies its draft as page-only; keep a copy before closing it.

**Stop application** or **Stop agent team** retains settings and saved state.
It does not remove separate knowledge services or billed cloud resources.

For a saved cloud computer, choose **Review cloud removal** when offered and
check its original VM, disk and access rule. Save anything you need first:
deleting the boot disk permanently loses its data, and Forge does not make a
backup. To proceed, acknowledge the disk-data loss and choose **Remove VM, disk
and access rule**. Keep the original removal request and choose **Recheck
removal** until its resources are confirmed absent. If removal is unconfirmed,
use its Google
Cloud management links to check the remaining resources; charges may continue.

If selection fails, the notice identifies the setup still selected;
**Retry selected setup** retries opening the requested one.

After an interrupted guide response, choose **Reload saved conversation** to
check its saved state without resending. If it is still working, wait; if it has
completed, inspect the result. If **Your submitted answer is saved** appears,
**Retry saved answer** deliberately resends the displayed request, including
its original files. **Review your draft** returns to the writing box without
changing your current writing, choices or files, and sends nothing. Retry still
uses the saved answer, even if you have since written something different.

If **Setup team could not start** appears, choose **Retry setup team** and wait
for its status. This starts the setup team while retaining the conversation;
it does not resend your answer.

For uncertain improvement submissions, use **Check Build status** to check
progress without resending. An open-page-only draft or receipt cannot survive
closing that page.

The builder's unsent note belongs to the selected setup. You can leave its build
panel or switch setups and return to the note. If browser storage fails, Forge
keeps your current edit in this open page and shows **Retry saving builder note**.
Use that action to try saving again; it does not send the note to the builder.
Copy the note before reloading or closing the page while that warning remains.

If a success report is unconfirmed, choose **Recheck success reports**. If
**Retry success confirmation** is available, it retries the earlier report
exactly; your current task draft remains separate. Keep the page open when its
report storage is unavailable.

If saving that report has not returned after 30 seconds, Forge releases the
wait and offers those recovery actions. The report may already be saved; recheck
before deciding to retry. Nothing is resent automatically, and your task and
problem-report drafts stay editable.

If Forge confirms that a report was not received, review the setup details and
retry deliberately with your retained draft. When browser storage is unavailable,
keep the page open and copy your task before reloading; this recovery cannot
clear a saved record that the browser cannot access.

**Advanced editor** opens manual editing before or after a proposal exists. On
the phone opening, find it below the optional examples; on a larger screen, use
the link below the preview. Returning without changes keeps your unsent idea.
**Configure
manually** also offers the editor in an error notice when the guide is unavailable.
Unchanged drafts submit nothing. If the plan changed while you edited,
**Review changes against current plan** deliberately
reviews your edits against the current proposal. If review is disabled, choose
**Adjust changes** to refine the retained draft.

Continue with the [full reference](README.md) for exported packages, credentials,
connection checks, background operation and supported update/recovery controls.
