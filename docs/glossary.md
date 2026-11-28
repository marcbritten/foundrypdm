# Portal glossary

This file gives one term for each thing that a user meets in the web portal. Use these terms in portal text, docs and messages (guideline W5 in [`ui-guidelines.md`](ui-guidelines.md)).

Each entry has three parts:

1. The term that portal text must use.
2. What the term means in Foundry PDM. The definitions come from the code and from the other files in `docs/`.
3. "Not:", the other words to avoid for the same thing. "None found" means that the portal uses no other word for it today.

Names in `code` are the API, database and code names. They do not change with the portal text.

## Vault

### Organization

The top-level account on a server. An organization has its own users, projects, items and settings. The server filters every read and write by the user's organization, so no user sees the data of another organization.

Not: company, tenant.

### Project

A named group of items in an organization. A project has folders, a cover image and, as an option, a main assembly.

Not: none found.

### Main assembly

The item that a project uses as the root of its bill of materials. The project also takes its cover image from this item.

Not: none found.

### Bill of materials

The list of the items that an assembly uses, with their quantities. The portal has a placeholder page for it. The server does not store the parent and child links yet.

Not: Bills of Material, Bill of material.

### Folder

A named place in a project that holds items and other folders. A folder can be inside another folder.

Not: directory.

### Item

What the vault keeps for one design, drawing or other file. An item has a part number, a category, a description, an owner, a lifecycle state and one file with its versions.

Not: part (for the item), document.

### Part

A physical piece that the shop makes, such as a sheet metal part. A part is always an item in the vault. Use "item" for what the vault keeps. Use "part" only where the text is about the physical piece or its flat pattern.

Not: component.

### Part number

The identifier of an item, such as `BE-000001`. The server makes it from the part-number prefix of the item's category and the next number for that prefix. The user who creates the item can type a part number instead. Part numbers are unique in the organization.

Not: PN, item number.

### Category

A named group of items. An admin adds categories and gives each one a part-number prefix in Settings, Part-number prefixes. The API and the database call this field `classification`.

The portal also offers four older codes in every category list: BE, ASM, MFG and SC. The code shows these uses for them. It does not say what the letters stand for.

- BE: the API default for a new item, and the category of a Part Studio that a user imports from Onshape.
- ASM: the category of an assembly that a user imports from Onshape. The main assembly picker lists ASM items first.
- MFG: the Manufacturing tab of a project lists the MFG items of that project. The Dashboard lists released MFG items as released manufacturing packages.
- SC: no server code sets it. The portal also shows "SC" as the file type badge of SolidCraft files.

Not: classification, class.

### Part-number prefix

The text at the start of each part number in a category. An admin sets one prefix for each category in Settings, Part-number prefixes. If a category has no prefix, the server uses the category name as the prefix.

Not: none found.

### File

The CAD model, drawing or other content that an item holds. Each item has one file. Each check-in adds a new version of that file.

Not: document, attachment.

### Property

A name and a value that a user stores on an item, such as a material or a finish. The item's Overview tab lists them under Properties.

Not: attribute, field (for a property), metadata.

### Owner

The user that an item names in its Owner field. The server sets the owner to the user who creates the item. An editor can change the owner with Assign owner.

Not: creator, author. A version has an author. An item has an owner.

### Preview

A picture of a version that the preview renderer makes after a check-in. The item lists and the Preview tab show it. An admin sets the preview style in Settings, Preview renderer.

Not: thumbnail.

### Design link

A link that opens one version of a design in the SolidCraft web editor. The link works without a sign-in. For this reason, the server signs each link and ends it after 10 minutes. Only a signed-in user of the same organization can get a design link.

Not: share link, public link.

### Derived STEP

A STEP file that a conversion job makes from a version. The job makes one for Inventor, FreeCAD, Rhino, STEP and IGES check-ins. The server keeps it beside the version and does not change the source file. The portal has no control for it yet. The API serves it at `GET /versions/{version_id}/derived-step`.

Not: STEP export, converted file.

## Versions and lifecycle

### Check out

To take the lock on an item. Only the user who holds the lock can check in the next version.

Not: lock (as a verb), reserve.

### Lock

The hold on an item that Check out makes. A lock names the user and the workstation, and it has an end time. A check-in or Undo checkout ends the lock. A viewer can end a lock that they hold.

Not: checkout (as a noun, except in the action name Undo checkout), lease.

### Undo checkout

The action that ends the user's own lock without a check-in. The item gets no new version.

Not: cancel checkout, unlock.

### Check in

To send a new file to an item as its next version. The user must hold the lock. The check-in ends the lock.

Not: commit, upload new version, save to PDM.

### Check-in note

The note that a user writes at check-in to say what changed. The Versions tab shows it. The API field is `commit_message`.

Not: commit message, message (for the note).

### Version

One saved state of an item's file. Each check-in adds a version and never changes an earlier one. The portal numbers the versions of an item in check-in order as v1, v2 and v3. This number is the `seq` field. It is not the "V" number of a major version.

Not: none found.

### Major version

A group of versions that starts with a design change. The portal writes it with a capital V, such as V2. The code calls it `file_versions.version`.

Not: none found.

### V2 - R3

The label that the check-in dialog and the item header show for a version. "V2" is the major version. "R3" is the third check-in in that major version. The code calls the R number `file_versions.revision`. The R number is not a revision label (see [Revision](#revision)).

Not: none found.

### Design change

A check-in that starts a new major version, for example from V2 - R3 to V3 - R1. The server marks the NC programs of the item as outdated and the nests that use it as stale.

Not: major revision, new version (for this choice).

### Minor update

A check-in that stays in the current major version, for example from V2 - R3 to V2 - R4. This check-in does not mark NC programs as outdated or nests as stale.

Not: revision (for this choice), minor revision.

### Revision

A label, such as A or B, that a release puts on one version. A revision never changes and never moves to another version. The labels go A, B, C unless the user who releases types another label. The Revisions tab lists them.

Not: rev, tag, release (for the label).

### Release

To mark the current version of an item as ready for use. A release puts a revision label on the version and sets the lifecycle state to Released. If the organization requires approval for releases, the release first makes an approval request and sets the state to In review. When an approver approves the request, the server uses the next label and the approver's note. It does not keep the label and the note that the user typed at release.

Not: publish, approve (for a release), sign off.

### Release note

The note that a user writes when they release an item. The Revisions tab shows it.

Not: message, comment (for the note).

### Lifecycle state

Where an item is in its life, such as WIP or Released. Each organization has a lifecycle that lists its states and the moves between them. The standard lifecycle has four states: WIP, In review, Released and Obsolete.

Not: status (for an item), stage, phase.

### WIP

Work in progress. A new item starts in WIP. An item also goes back to WIP when an approver rejects its release.

Not: in work.

### In review

The release of the item waits for an approval request. In the portal, only a release sets this state, and only when the organization requires approval for releases.

Not: pending, awaiting approval.

### Released

In the portal, a release sets this state, or an approver approves the approval request. The item then has a revision label. The Dashboard counts released items.

Not: approved (for the state), published.

### Obsolete

The last state of the standard lifecycle. An editor moves a released item to Obsolete with Mark obsolete. The standard lifecycle has no move out of Obsolete.

Not: retired, deprecated.

### Draft

The portal has a label for a Draft state. The standard lifecycle does not have this state, and no server code sets it.

Not: none found.

### Approval request

A request to approve the release of an item. A release makes one when the organization requires approval for releases. An admin turns this on in Settings, Release approval. The request goes to the approvers that the release names, or else to every editor and admin. An approver approves or rejects the request on the Approvals page.

Not: approval (for the request), task.

## Manufacturing and nesting

### Machine

A record for one machine in the shop, such as a CNC router, a laser cutter or a 3D printer. The Machines & Tools page lists the machines.

Not: device, station.

### Tool

A record for one cutting tool, such as an end mill or a drill. A tool has its material, its geometry and its cutting data.

Not: cutter, bit.

### Tool library

All the tools of an organization. The Tool library page lists them.

Not: tool database.

### Post-processor

A record for the step that writes the NC program (G-code) for one machine or controller. An NC program records which post-processor made it and the version of that post-processor.

Not: post, postprocessor.

### Setup

The record that connects a version of a CAD model to CAM. A setup holds the stock, the workholding and the work coordinate system.

Not: manufacturing setup, mfg setup.

### CAM project

The record that holds the toolpaths for a part.

Not: CAM job, CAM file.

### NC program

The G-code for one machine and one post-processor. An NC program records the source version, the machine and the post-processor version. When a user checks in a design change to the source item, the server marks the NC program as outdated. An editor can release an NC program. This release is separate from the release of an item.

Not: G-code file, toolpath (for the program), program (alone).

### Production job

A run of one item on one machine from one NC program. A production job can name who runs the machine. Users with the operator role or a higher role record its progress from the shop floor.

Not: job (alone), work order, task.

### Print job

A 3D printing job for one item on one printer. Users with the operator role or a higher role record its progress.

Not: job (alone).

### Inspection

The record of one check of a production job or an item. An inspection holds pass or fail, notes and the measured values. Users with the operator role or a higher role record inspections.

Not: QC.

### Sheet metal spec

The sheet metal data of one item: material, thickness, bend radius, K-factor and, when there is one, the flat pattern. The item's Sheet metal tab shows it.

Not: sheet metal settings.

### Sheet metal standard

A shared record of bend data for one material at one thickness. The Sheet Metal Standards page lists them. The Sheet metal tab of an item offers their material names.

Not: sheet metal spec (for the shared record).

### Flat pattern

The flat outline of a sheet metal part, as a DXF file. A background job makes it from a SolidCraft sheet metal design, or a user uploads it. Nests use flat patterns.

Not: unfold, blank, DXF (alone).

### Nest

A layout of flat patterns on one or more sheets. The nesting engine makes it in a background job. A nest keeps the exact versions and flat patterns that it used. Re-run makes a new nest from the same parts. When a user checks in a design change to a source item, the server marks the nest as stale and does not change it. The Laser Nests page lists the nests.

Not: nest run, nesting run, layout (alone).

### Sheet

One piece of rectangular stock in a nest. The nest exports one DXF file and one preview for each sheet.

Not: plate, panel.

### Background job

Work that the server's worker does after a request, such as a file conversion, a preview, an import or a nest. The Jobs page lists the latest 100 background jobs.

Not: task, job (alone).

## People and access

### User

A person with an account in one organization. Each user has one role.

Not: member, account (for the person).

### Role

The level of access of a user in the organization. The four roles are viewer, operator, editor and admin. Each role can do all that the roles before it can do. An admin sets roles in Administration, Users.

Not: permission, group.

### Viewer

A user who reads the projects, items, files and records of the organization, and adds comments. A viewer can open design links, end a lock that they hold and decide an approval request that names them.

Not: guest.

### Operator

A viewer who also records shop floor work: the progress of production jobs and print jobs, and inspections.

Not: none found.

### Editor

An operator who also changes the vault. An editor changes projects, items and properties. An editor also checks items out and in, uploads files, releases items, changes lifecycle states, and archives and deletes items. Editors change tools, machines, post-processors and manufacturing records, and they can decide any approval request.

Not: none found.

### Admin

An editor who also manages users, API tokens and the settings of the organization.

Not: administrator (in labels).

### Host admin

An admin who can also change what all organizations on the server share. These shared things are the CAD CLI, the SolidCraft web editor, the Dutchman release channel and the GPU policy of the preview renderer. In the portal, only a host admin can make another admin a host admin. The server does not let anyone revoke the last host admin.

Not: super admin, server admin.

### API token

A secret that a CAD connector, an agent or a script uses in place of a password. An admin makes an API token for a user in Administration, Users. An API token stays valid until a user deletes it.

Not: key, access token.

### Session

A sign-in with a password. By default a session ends after 120 minutes with no request, or 720 minutes after sign-in. The server settings `PDM_SESSION_IDLE_MINUTES` and `PDM_SESSION_LIFETIME_MINUTES` change these limits. A session also ends when the user signs out. When a session ends, the portal asks the user to sign in again.

Not: web sign-in, login, token (for a session).

## Settings and appearance

### Branding

The name, logo and color changes of an organization. An admin sets them in Settings, Branding. The sign-in page and the sidebar show the name and the logo. The color changes apply on top of each theme.

Not: theme (for branding), skin, white label.

### Theme

The color set of the portal in one browser: Dark, Light, Slate or High contrast. Dark is the default. Each user picks a theme in Settings, Appearance.

Not: color scheme, skin, mode.

### Reading font

The font for menus, tables, forms and messages. The choices are System default and Atkinson Hyperlegible Next. Each user picks it for one browser. The default follows the organization default, which an admin sets.

Not: UI font, body font, typeface.

### Code font

The font for part numbers, revision labels, file hashes and other codes. The choices are System default and Atkinson Hyperlegible Mono. It follows the same rules as the reading font.

Not: mono font, monospace.

### Text size

The size of all text in the portal in one browser: Default (100%), Larger (112.5%) or Largest (125%). Default follows the text size of the browser.

Not: zoom, font size, scale.

### Density

How much padding table rows and lists have in one browser: Comfortable or Compact. The top bar has a button that changes it.

Not: view mode.
