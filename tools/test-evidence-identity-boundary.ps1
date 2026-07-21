[CmdletBinding()]param()
Set-StrictMode -Version Latest;$ErrorActionPreference='Stop'
$repoRoot=(& git rev-parse --show-toplevel 2>$null).Trim();if($LASTEXITCODE-ne0){throw'Run inside repository.'}
$checker=Join-Path $repoRoot 'tools/check-evidence-identity-boundary.ps1';$baseline=Get-Content -Raw (Join-Path $repoRoot 'contracts/evidence/identity-boundary.json')|ConvertFrom-Json -Depth 100
$tempBase=[IO.Path]::GetFullPath([IO.Path]::GetTempPath());$tempRoot=[IO.Path]::GetFullPath((Join-Path $tempBase ('openloops-evidence-'+[guid]::NewGuid().ToString('N'))));if(-not$tempRoot.StartsWith($tempBase,[StringComparison]::OrdinalIgnoreCase)){throw'Unsafe temp path.'};[void](New-Item -ItemType Directory $tempRoot);$caseNumber=0
function Case([string]$Name,[string]$Expected,[scriptblock]$Mutate){$script:caseNumber++;$c=$baseline|ConvertTo-Json -Depth 100|ConvertFrom-Json -Depth 100;&$Mutate $c;$path=Join-Path $tempRoot "case-$caseNumber.json";$c|ConvertTo-Json -Depth 100|Set-Content $path -Encoding utf8NoBOM;$out=(&pwsh -NoProfile -File $checker -ManifestPath $path -Quiet 2>&1|Out-String);if($LASTEXITCODE-eq0-or$out-notmatch[regex]::Escape($Expected)){throw "Synthetic evidence case failed to trigger $Expected`: $Name"}}
function DocCase([string]$Name,[string]$Expected,[string]$Source,[string]$Arg,[scriptblock]$Mutate){$script:caseNumber++;$t=Get-Content -Raw (Join-Path $repoRoot $Source);$t=&$Mutate $t;$path=Join-Path $tempRoot ("doc-$caseNumber"+[IO.Path]::GetExtension($Source));Set-Content $path $t -Encoding utf8NoBOM -NoNewline;$out=(&pwsh -NoProfile -File $checker $Arg $path -Quiet 2>&1|Out-String);if($LASTEXITCODE-eq0-or$out-notmatch[regex]::Escape($Expected)){throw "Synthetic evidence document case failed to trigger $Expected`: $Name"}}
function WorkspaceCase([string]$Name,[scriptblock]$Mutate){$script:caseNumber++;$root=Join-Path $tempRoot "workspace-$caseNumber";[void](New-Item -ItemType Directory $root);foreach($name in @('.node-version','Cargo.toml','Cargo.lock','eslint.config.mjs','package.json','package-lock.json','rust-toolchain.toml')){Copy-Item -LiteralPath (Join-Path $repoRoot $name) -Destination $root};Copy-Item -LiteralPath (Join-Path $repoRoot 'crates') -Destination $root -Recurse;[void](New-Item -ItemType Directory (Join-Path $root 'outlook-addin'));Copy-Item -LiteralPath (Join-Path $repoRoot 'outlook-addin\src') -Destination (Join-Path $root 'outlook-addin') -Recurse;Copy-Item -LiteralPath (Join-Path $repoRoot 'outlook-addin\test') -Destination (Join-Path $root 'outlook-addin') -Recurse;Copy-Item -LiteralPath (Join-Path $repoRoot 'outlook-addin\tsconfig.json') -Destination (Join-Path $root 'outlook-addin');[void](New-Item -ItemType Directory (Join-Path $root 'contracts\ipc') -Force);Copy-Item -LiteralPath (Join-Path $repoRoot 'contracts\ipc\skeleton-status.schema.json') -Destination (Join-Path $root 'contracts\ipc');[void](New-Item -ItemType Directory (Join-Path $root 'tools'));Copy-Item -LiteralPath (Join-Path $repoRoot 'tools\generate-contracts.mjs') -Destination (Join-Path $root 'tools');&$Mutate $root;$out=(&pwsh -NoProfile -File $checker -WorkspaceScanRoot $root -Quiet 2>&1|Out-String);if($LASTEXITCODE-eq0-or$out-notmatch'P0-EVID-CLAIMS-001'){throw "Synthetic evidence workspace case failed to trigger P0-EVID-CLAIMS-001`: $Name"}}
try{
 &pwsh -NoProfile -File $checker -Quiet;if($LASTEXITCODE){throw'Valid evidence boundary failed.'}
 Case 'unknown field' 'P0-EVID-INVENTORY-001' {param($c)$c|Add-Member extra synthetic}
 Case 'work item drift' 'P0-EVID-INVENTORY-001' {param($c)$c.work_item='P0-WI-X'}
 Case 'repair item drift' 'P0-EVID-INVENTORY-001' {param($c)$c.repair_item='P0-WI-XR'}
 Case 'repair authorization widened' 'P0-EVID-INVENTORY-001' {param($c)$c.repair_authorization='enable runtime'}
 Case 'owner omitted' 'P0-EVID-INVENTORY-001' {param($c)$c.owner_decisions=@('OWN-07')}
 Case 'owner order changed' 'P0-EVID-INVENTORY-001' {param($c)$c.owner_decisions=@('OWN-07','OWN-05')}
 Case 'requirement omitted' 'P0-EVID-INVENTORY-001' {param($c)$c.requirements=@($c.requirements|Select-Object -Skip 1)}
 Case 'scenario added' 'P0-EVID-INVENTORY-001' {param($c)$c.acceptance_scenarios+=@('AS-01')}
 Case 'gate omitted' 'P0-EVID-INVENTORY-001' {param($c)$c.blocking_gates=@($c.blocking_gates|Where-Object{$_ -ne 'G-PRIV'})}
 Case 'source variant omitted' 'P0-EVID-INVENTORY-001' {param($c)$c.closed_source_variants=@($c.closed_source_variants|Select-Object -Skip 1)}
 Case 'source variant enabled' 'P0-EVID-INVENTORY-001' {param($c)$c.closed_source_variants[0].state='enabled'}
 Case 'source variant advertised' 'P0-EVID-INVENTORY-001' {param($c)$c.closed_source_variants[1].advertised=$true}

 Case 'schema record drift' 'P0-EVID-SCHEMA-001' {param($c)$c.email_record_schema.record_type='generic_source'}
 Case 'schema unknown keys allowed' 'P0-EVID-SCHEMA-001' {param($c)$c.email_record_schema.unknown_or_duplicate_fields='ignore'}
 Case 'schema field omitted' 'P0-EVID-SCHEMA-001' {param($c)$c.email_record_schema.fields_in_canonical_order=@($c.email_record_schema.fields_in_canonical_order|Where-Object{$_ -ne 'account_ref'})}
 Case 'schema field order swapped' 'P0-EVID-SCHEMA-001' {param($c)$x=$c.email_record_schema.fields_in_canonical_order[0];$c.email_record_schema.fields_in_canonical_order[0]=$c.email_record_schema.fields_in_canonical_order[1];$c.email_record_schema.fields_in_canonical_order[1]=$x}
 Case 'field contract omitted' 'P0-EVID-SCHEMA-001' {param($c)$c.email_record_schema.field_contracts=@($c.email_record_schema.field_contracts|Where-Object field -ne 'fallback_locator')}
 Case 'field made optional' 'P0-EVID-SCHEMA-001' {param($c)($c.email_record_schema.field_contracts|Where-Object field -eq 'observed_at').required=$false}
 Case 'range changed to bytes' 'P0-EVID-SCHEMA-001' {param($c)($c.email_record_schema.field_contracts|Where-Object field -eq 'range_start').logical_type='byte_offset'}
 Case 'range end inclusive' 'P0-EVID-SCHEMA-001' {param($c)($c.email_record_schema.field_contracts|Where-Object field -eq 'range_end').logical_type='u32_unicode_scalar_index_inclusive'}
 Case 'digest truncated' 'P0-EVID-SCHEMA-001' {param($c)($c.email_record_schema.field_contracts|Where-Object field -eq 'content_digest_hmac').exact_bytes=16}
 Case 'component added' 'P0-EVID-SCHEMA-001' {param($c)($c.email_record_schema.field_contracts|Where-Object field -eq 'component_code').catalog+=@('free_text')}
 Case 'relation omitted' 'P0-EVID-SCHEMA-001' {param($c)$f=$c.email_record_schema.field_contracts|Where-Object field -eq 'relation_code';$f.catalog=@($f.catalog|Where-Object{$_ -ne 'closure'})}
 Case 'cross-field range rule removed' 'P0-EVID-SCHEMA-001' {param($c)$c.email_record_schema.cross_field_rules=@($c.email_record_schema.cross_field_rules|Where-Object{$_-notmatch'range_start is strictly'})}

 Case 'primary default ID' 'P0-EVID-IDENTITY-001' {param($c)$c.locator_contract.primary.kind='graph_rest_id'}
 Case 'primary ID plaintext' 'P0-EVID-IDENTITY-001' {param($c)$c.locator_contract.primary.immutable_id.storage='plaintext_index'}
 Case 'folder becomes identity' 'P0-EVID-IDENTITY-001' {param($c)$c.locator_contract.primary.not_identity=@($c.locator_contract.primary.not_identity|Where-Object{$_ -ne 'folder'})}
 Case 'content digest becomes identity' 'P0-EVID-IDENTITY-001' {param($c)$c.locator_contract.primary.not_identity=@($c.locator_contract.primary.not_identity|Where-Object{$_ -ne 'content_digest'})}
 Case 'mailbox binding omitted' 'P0-EVID-IDENTITY-001' {param($c)$c.locator_contract.primary.fields_in_canonical_order=@('kind','account_ref','immutable_id')}
 Case 'fallback authoritative' 'P0-EVID-IDENTITY-001' {param($c)$c.locator_contract.fallback.authority='proves identity'}
 Case 'unknown fallback source' 'P0-EVID-IDENTITY-001' {param($c)$c.locator_contract.fallback.source_codes+=@('internet_message_id')}
 Case 'cross-account request allowed' 'P0-EVID-IDENTITY-001' {param($c)$c.locator_contract.account_binding.cross_account_rule='continue request'}
 Case 'mailbox HMAC algorithm weakened' 'P0-EVID-IDENTITY-001' {param($c)$c.locator_contract.account_binding.mailbox_binding_hmac.algorithm='SHA-256'}
 Case 'mailbox HMAC key reused' 'P0-EVID-IDENTITY-001' {param($c)$c.locator_contract.account_binding.mailbox_binding_hmac.key='AEAD key'}
 Case 'mailbox HMAC purpose changed' 'P0-EVID-IDENTITY-001' {param($c)$c.locator_contract.account_binding.mailbox_binding_hmac.purpose_tag='synthetic'}
 Case 'mailbox HMAC schema changed' 'P0-EVID-IDENTITY-001' {param($c)$c.locator_contract.account_binding.mailbox_binding_hmac.owning_schema_version=2}
 Case 'mailbox HMAC component count changed' 'P0-EVID-IDENTITY-001' {param($c)$c.locator_contract.account_binding.mailbox_binding_hmac.component_count=2}
 Case 'mailbox HMAC layout weakened' 'P0-EVID-IDENTITY-001' {param($c)$c.locator_contract.account_binding.mailbox_binding_hmac.input_layout='concatenate claims'}
 Case 'mailbox claim source widened' 'P0-EVID-IDENTITY-001' {param($c)$c.locator_contract.account_binding.mailbox_binding_hmac.claim_source='address string'}
 Case 'mailbox HMAC truncated' 'P0-EVID-IDENTITY-001' {param($c)$c.locator_contract.account_binding.mailbox_binding_hmac.output_bytes=16}
 Case 'mailbox comparison partial' 'P0-EVID-IDENTITY-001' {param($c)$c.locator_contract.account_binding.mailbox_binding_hmac.comparison='prefix'}
 Case 'raw mailbox claims persisted' 'P0-EVID-IDENTITY-001' {param($c)$c.locator_contract.account_binding.mailbox_binding_hmac.storage='persist tid oid'}

 Case 'beta API selected' 'P0-EVID-GRAPH-001' {param($c)$c.graph_request_contract.api_version='beta'}
 Case 'folder-qualified refetch' 'P0-EVID-GRAPH-001' {param($c)$c.graph_request_contract.refetch_path_shape='/me/mailFolders/{folder}/messages/{id}'}
 Case 'immutable preference removed' 'P0-EVID-GRAPH-001' {param($c)$c.graph_request_contract.immutable_preference_header='none'}
 Case 'immutable preference not continued' 'P0-EVID-GRAPH-001' {param($c)$c.graph_request_contract.preference_rule='initial only'}
 Case 'body preview selected' 'P0-EVID-GRAPH-001' {param($c)$c.graph_request_contract.delta_select_fields_in_order+=@('bodyPreview')}
 Case 'selected field order changed' 'P0-EVID-GRAPH-001' {param($c)$x=$c.graph_request_contract.delta_select_fields_in_order[0];$c.graph_request_contract.delta_select_fields_in_order[0]=$c.graph_request_contract.delta_select_fields_in_order[1];$c.graph_request_contract.delta_select_fields_in_order[1]=$x}
 Case 'prohibited field removed' 'P0-EVID-GRAPH-001' {param($c)$c.graph_request_contract.prohibited_selected_fields=@($c.graph_request_contract.prohibited_selected_fields|Where-Object{$_ -ne 'internetMessageId'})}
 Case 'attachment content allowed' 'P0-EVID-GRAPH-001' {param($c)$c.graph_request_contract.attachment_content_fetch='allowed'}
 Case 'attachment path omitted' 'P0-EVID-GRAPH-001' {param($c)$c.graph_request_contract.attachment_path_shape='none'}
 Case 'attachment method changed' 'P0-EVID-GRAPH-001' {param($c)$c.graph_request_contract.attachment_method='POST'}
 Case 'attachment query fetches bytes' 'P0-EVID-GRAPH-001' {param($c)$c.graph_request_contract.attachment_query_shape='$select=id,name,contentBytes'}
 Case 'attachment enumeration trusts hasAttachments' 'P0-EVID-GRAPH-001' {param($c)$c.graph_request_contract.attachment_enumeration_rule='enumerate only when hasAttachments is true'}
 Case 'attachment source omitted' 'P0-EVID-GRAPH-001' {param($c)$c.sources=@($c.sources|Where-Object id -ne 'SRC-MS-MESSAGE-ATTACHMENTS')}
 Case 'translate endpoint enabled' 'P0-EVID-GRAPH-001' {param($c)$c.graph_request_contract.translation_endpoint.state='enabled'}
 Case 'translate scope widened' 'P0-EVID-GRAPH-001' {param($c)$c.graph_request_contract.translation_endpoint.reason='request User.Read'}
 Case 'scope failure owner changed' 'P0-EVID-GRAPH-001' {param($c)$c.graph_request_contract.translation_endpoint.owner_route='engineering'}

 Case 'anchor UTF-16 range' 'P0-EVID-ANCHOR-001' {param($c)$c.canonical_anchor_contract.range_unit='UTF-16 code unit'}
 Case 'anchor end inclusive' 'P0-EVID-ANCHOR-001' {param($c)$c.canonical_anchor_contract.range_end='inclusive'}
 Case 'empty anchor allowed' 'P0-EVID-ANCHOR-001' {param($c)$c.canonical_anchor_contract.empty_ranges='allowed'}
 Case 'Unicode normalization removed' 'P0-EVID-ANCHOR-001' {param($c)$c.canonical_anchor_contract.normalization_steps_in_order=@($c.canonical_anchor_contract.normalization_steps_in_order|Where-Object{$_ -ne 'normalize Unicode to NFC'})}
 Case 'normalization order swapped' 'P0-EVID-ANCHOR-001' {param($c)$x=$c.canonical_anchor_contract.normalization_steps_in_order[2];$c.canonical_anchor_contract.normalization_steps_in_order[2]=$c.canonical_anchor_contract.normalization_steps_in_order[3];$c.canonical_anchor_contract.normalization_steps_in_order[3]=$x}
 Case 'prefix crosses block' 'P0-EVID-ANCHOR-001' {param($c)$c.canonical_anchor_contract.context_edges='cross blocks'}
 Case 'prefix shortened' 'P0-EVID-ANCHOR-001' {param($c)$c.canonical_anchor_contract.prefix_context_scalars=16}
 Case 'digest unkeyed' 'P0-EVID-ANCHOR-001' {param($c)$c.canonical_anchor_contract.digest_algorithm='SHA-256'}
 Case 'digest truncated' 'P0-EVID-ANCHOR-001' {param($c)$c.canonical_anchor_contract.digest_output_bytes=16}
 Case 'digest compare partial' 'P0-EVID-ANCHOR-001' {param($c)$c.canonical_anchor_contract.comparison='prefix compare'}
 Case 'digest identity authority' 'P0-EVID-ANCHOR-001' {param($c)$c.canonical_anchor_contract.digest_non_authority='proves item identity'}
 Case 'observation HMAC output truncated' 'P0-EVID-ANCHOR-001' {param($c)$c.observation_digest_contract.outputs='16 bytes'}
 Case 'changeKey may persist' 'P0-EVID-ANCHOR-001' {param($c)$c.observation_digest_contract.transient_inputs='may log changeKey'}
 Case 'observation repeats outer HMAC frame' 'P0-EVID-ANCHOR-001' {param($c)$c.observation_digest_contract.framing_rule='repeat purpose tag and account binding'}
 Case 'anchor repeats outer HMAC frame' 'P0-EVID-ANCHOR-001' {param($c)$c.canonical_anchor_contract.framing='repeat purpose tag account binding then components'}
 Case 'observation component count drift' 'P0-EVID-ANCHOR-001' {param($c)$c.observation_digest_contract.component_counts.query_fingerprint_v1=8}
 Case 'anchor component count drift' 'P0-EVID-ANCHOR-001' {param($c)$c.canonical_anchor_contract.component_count=8}
 Case 'component numeric map omitted' 'P0-EVID-ANCHOR-001' {param($c)$c.canonical_anchor_contract.component_code_map=@($c.canonical_anchor_contract.component_code_map|Select-Object -Skip 1)}
 Case 'component numeric code duplicated' 'P0-EVID-ANCHOR-001' {param($c)$c.canonical_anchor_contract.component_code_map[1].code=1}
 Case 'component numeric map reordered' 'P0-EVID-ANCHOR-001' {param($c)$x=$c.canonical_anchor_contract.component_code_map[0];$c.canonical_anchor_contract.component_code_map[0]=$c.canonical_anchor_contract.component_code_map[1];$c.canonical_anchor_contract.component_code_map[1]=$x}
 Case 'observation direction map omitted' 'P0-EVID-ANCHOR-001' {param($c)$c.observation_digest_contract.direction_code_map=@($c.observation_digest_contract.direction_code_map|Select-Object -Skip 1)}
 Case 'observation read code duplicated' 'P0-EVID-ANCHOR-001' {param($c)$c.observation_digest_contract.read_observation_code_map[1].code=1}
 Case 'first observation map reordered' 'P0-EVID-ANCHOR-001' {param($c)$x=$c.observation_digest_contract.first_observation_code_map[0];$c.observation_digest_contract.first_observation_code_map[0]=$c.observation_digest_contract.first_observation_code_map[1];$c.observation_digest_contract.first_observation_code_map[1]=$x}
 Case 'content seventh component removed' 'P0-EVID-ANCHOR-001' {param($c)$c.canonical_anchor_contract.purpose_layouts[0].seventh_component='none'}
 Case 'prefix seventh component widened' 'P0-EVID-ANCHOR-001' {param($c)$c.canonical_anchor_contract.purpose_layouts[1].seventh_component='all preceding blocks'}
 Case 'suffix purpose omitted' 'P0-EVID-ANCHOR-001' {param($c)$c.canonical_anchor_contract.purpose_layouts=@($c.canonical_anchor_contract.purpose_layouts|Select-Object -First 2)}
 Case 'Graph body shape widened' 'P0-EVID-ANCHOR-001' {param($c)$c.canonical_projection_contract.graph_shape_rules.body='accept any body'}
 Case 'participant encoding ambiguous' 'P0-EVID-ANCHOR-001' {param($c)$c.canonical_projection_contract.participant_text_rule='concatenate name and address'}
 Case 'HTML parser unpinned' 'P0-EVID-ANCHOR-001' {param($c)$c.canonical_projection_contract.html_text_profile.parser='any parser'}
 Case 'HTML drop list removed' 'P0-EVID-ANCHOR-001' {param($c)$c.canonical_projection_contract.html_text_profile.drop_elements_with_descendants=@()}
 Case 'HTML quote segmentation weakened' 'P0-EVID-ANCHOR-001' {param($c)$c.canonical_projection_contract.html_text_profile.quote_segmentation='flatten all text'}
 Case 'HTML link href read' 'P0-EVID-ANCHOR-001' {param($c)$c.canonical_projection_contract.html_text_profile.link_labels='include href'}
 Case 'HTML remote behavior enabled' 'P0-EVID-ANCHOR-001' {param($c)$c.canonical_projection_contract.html_text_profile.remote_behavior='load images'}
 Case 'DOM traversal order removed' 'P0-EVID-ANCHOR-001' {param($c)$c.canonical_projection_contract.html_text_profile.dom_event_algorithm='walk elements in any order'}
 Case 'DOM exit separator removed' 'P0-EVID-ANCHOR-001' {param($c)$c.canonical_projection_contract.html_text_profile.dom_event_algorithm=$c.canonical_projection_contract.html_text_profile.dom_event_algorithm-replace' and exactly one LF immediately after visiting its last child',''}
 Case 'table cell separators removed' 'P0-EVID-ANCHOR-001' {param($c)$c.canonical_projection_contract.html_text_profile.dom_event_algorithm=$c.canonical_projection_contract.html_text_profile.dom_event_algorithm-replace'For every td or th','For no table cell'}
 Case 'outer quote emits boundary' 'P0-EVID-ANCHOR-001' {param($c)$c.canonical_projection_contract.html_text_profile.outermost_blockquote_algorithm='emit ordinary entry and exit then quote'}
 Case 'LF finalization widened' 'P0-EVID-ANCHOR-001' {param($c)$c.canonical_projection_contract.html_text_profile.line_feed_finalization='implementation-defined whitespace'}
 Case 'canonical vector omitted' 'P0-EVID-ANCHOR-001' {param($c)$c.canonical_projection_contract.conformance_vectors=@($c.canonical_projection_contract.conformance_vectors|Select-Object -Skip 1)}
 Case 'sibling block vector changed' 'P0-EVID-ANCHOR-001' {param($c)($c.canonical_projection_contract.conformance_vectors|Where-Object id -eq 'CANON-HTML-SIBLING-BLOCKS-001').body_blocks=@("A`nB")}
 Case 'nested block vector changed' 'P0-EVID-ANCHOR-001' {param($c)($c.canonical_projection_contract.conformance_vectors|Where-Object id -eq 'CANON-HTML-NESTED-BLOCKS-001').body_blocks=@('ABC')}
 Case 'table vector changed' 'P0-EVID-ANCHOR-001' {param($c)($c.canonical_projection_contract.conformance_vectors|Where-Object id -eq 'CANON-HTML-TABLE-CELLS-001').body_blocks=@('AB')}
 Case 'inline-only attachment rejected' 'P0-EVID-ANCHOR-001' {param($c)($c.canonical_projection_contract.conformance_vectors|Where-Object id -eq 'CANON-INLINE-ONLY-ATTACHMENT-001').result='skip'}
 Case 'canonical attachment shape trusts hasAttachments' 'P0-EVID-ANCHOR-001' {param($c)$c.canonical_projection_contract.graph_shape_rules.attachments='only when hasAttachments true'}
 Case 'surrogate vector accepted' 'P0-EVID-ANCHOR-001' {param($c)($c.canonical_projection_contract.conformance_vectors|Where-Object id -eq 'CANON-UNPAIRED-SURROGATE-001').result='accept'}
 Case 'noncharacter policy removed' 'P0-EVID-ANCHOR-001' {param($c)$c.canonical_projection_contract.scalar_policy.rejected_scalars='none'}

 Case 'resolution state omitted' 'P0-EVID-RESOLUTION-001' {param($c)$c.resolution_state_machine.states=@($c.resolution_state_machine.states|Where-Object{$_ -ne 'candidate_ambiguous'})}
 Case 'resolution step omitted' 'P0-EVID-RESOLUTION-001' {param($c)$c.resolution_state_machine.steps_in_order=@($c.resolution_state_machine.steps_in_order|Select-Object -Skip 1)}
 Case 'network before binding' 'P0-EVID-RESOLUTION-001' {param($c)$c.resolution_state_machine.steps_in_order[0]='make request then bind'}
 Case 'prefix suffix not required' 'P0-EVID-RESOLUTION-001' {param($c)$c.resolution_state_machine.steps_in_order[2]='require content digest'}
 Case 'multiple candidate auto pick' 'P0-EVID-RESOLUTION-001' {param($c)$c.resolution_state_machine.steps_in_order[6]='pick first'}
 Case 'automatic reanchor' 'P0-EVID-RESOLUTION-001' {param($c)$c.resolution_state_machine.automatic_reanchoring='allowed'}
 Case 'stored content fallback' 'P0-EVID-RESOLUTION-001' {param($c)$c.resolution_state_machine.stored_content_fallback='allowed'}
 Case 'missing becomes negative' 'P0-EVID-RESOLUTION-001' {param($c)$c.resolution_state_machine.missing_is_not_negative_evidence=$false}
 Case 'automation continues unavailable' 'P0-EVID-RESOLUTION-001' {param($c)$c.resolution_state_machine.evidence_dependent_automation_on_changed_ambiguous_unavailable='continue'}
 Case 'incomplete search offers singleton' 'P0-EVID-RESOLUTION-001' {param($c)$c.fallback_search_contract.incomplete_classification='offer one observed match'}
 Case 'candidate universe widened' 'P0-EVID-RESOLUTION-001' {param($c)$c.fallback_search_contract.candidate_universe='search every mailbox'}
 Case 'terminal page not required' 'P0-EVID-RESOLUTION-001' {param($c)$c.fallback_search_contract.completion_marker='complete after first match'}

 Case 'same mailbox move assumed' 'P0-EVID-LIFECYCLE-001' {param($c)$c.lifecycle_identity_rules.same_mailbox_move='always valid'}
 Case 'copy merged' 'P0-EVID-LIFECYCLE-001' {param($c)$c.lifecycle_identity_rules.copy='merge by digest'}
 Case 'draft creates evidence' 'P0-EVID-LIFECYCLE-001' {param($c)$c.lifecycle_identity_rules.draft_or_compose='create source'}
 Case 'sent means delivered' 'P0-EVID-LIFECYCLE-001' {param($c)$c.lifecycle_identity_rules.saved_sent_copy='create and claim delivery'}
 Case 'Internet Message-ID identity' 'P0-EVID-LIFECYCLE-001' {param($c)$c.lifecycle_identity_rules.internet_message_id='primary identity'}
 Case 'known loss fabricated' 'P0-EVID-LIFECYCLE-001' {param($c)$c.lifecycle_identity_rules.send_without_saved_copy_or_immediate_move_delete='fabricate locator'}
 Case 'ordinary archive assumed safe' 'P0-EVID-LIFECYCLE-001' {param($c)$c.lifecycle_identity_rules.ordinary_archive_folder_move='always safe'}
 Case 'archive mailbox conflated' 'P0-EVID-LIFECYCLE-001' {param($c)$c.lifecycle_identity_rules.in_place_archive_mailbox='same as Archive folder'}

 Case 'Office Graph authority merged' 'P0-EVID-OFFICE-001' {param($c)$c.office_and_link_contract.authority_separation='Office grants Graph'}
 Case 'Office ID persisted' 'P0-EVID-OFFICE-001' {param($c)$c.office_and_link_contract.office_item_id='persist EWS id'}
 Case 'wrong REST version' 'P0-EVID-OFFICE-001' {param($c)$c.office_and_link_contract.office_to_graph=$c.office_and_link_contract.office_to_graph-replace'v2_0','v1_0'}
 Case 'Graph-to-Office assumed' 'P0-EVID-OFFICE-001' {param($c)$c.office_and_link_contract.graph_to_office='available'}
 Case 'translate as Office fallback' 'P0-EVID-OFFICE-001' {param($c)$c.office_and_link_contract.graph_to_office='use translateExchangeIds as a fallback'}
 Case 'web link host widened' 'P0-EVID-OFFICE-001' {param($c)$c.office_and_link_contract.web_link_origin_allowlist+=@('https://synthetic.invalid')}
 Case 'web link persisted' 'P0-EVID-OFFICE-001' {param($c)$c.office_and_link_contract.graph_web_link='persist and iframe'}
 Case 'review link contains Graph ID' 'P0-EVID-OFFICE-001' {param($c)$c.office_and_link_contract.review_link='contains Graph ID'}
 Case 'synchronous display allowed' 'P0-EVID-OFFICE-001' {param($c)$c.office_and_link_contract.display_message_form='call displayMessageForm'}
 Case 'web URL credentials allowed' 'P0-EVID-OFFICE-001' {param($c)$c.office_and_link_contract.web_link_validation.credentials='allowed'}
 Case 'web nondefault port allowed' 'P0-EVID-OFFICE-001' {param($c)$c.office_and_link_contract.web_link_validation.port='any HTTPS port'}
 Case 'web trailing dot allowed' 'P0-EVID-OFFICE-001' {param($c)$c.office_and_link_contract.web_link_validation.host='suffix match'}
 Case 'web controls allowed' 'P0-EVID-OFFICE-001' {param($c)$c.office_and_link_contract.web_link_validation.preparse='trim input'}
 Case 'web origin widened after parse' 'P0-EVID-OFFICE-001' {param($c)$c.office_and_link_contract.web_link_validation.origin='host suffix'}

 Case 'unavailable code omitted' 'P0-EVID-UNAVAILABLE-001' {param($c)$c.unavailable_projection.safe_codes=@($c.unavailable_projection.safe_codes|Where-Object{$_ -ne 'evidence_unavailable'})}
 Case 'unavailable subject projection' 'P0-EVID-UNAVAILABLE-001' {param($c)$c.unavailable_projection.prohibited_projection=@($c.unavailable_projection.prohibited_projection|Where-Object{$_ -ne 'subject'})}
 Case 'recovery action omitted' 'P0-EVID-UNAVAILABLE-001' {param($c)$c.unavailable_projection.user_actions=@($c.unavailable_projection.user_actions|Where-Object{$_ -ne 'completed_outside_email'})}
 Case 'unavailable closes' 'P0-EVID-UNAVAILABLE-001' {param($c)$c.unavailable_projection.closure_rule='close loop'}

 Case 'human text allowed' 'P0-EVID-PRIVACY-001' {param($c)$c.privacy_boundary.human_readable_mailbox_or_generated_text='allowed encrypted'}
 Case 'diagnostic event allowed' 'P0-EVID-PRIVACY-001' {param($c)$c.privacy_boundary.diagnostic_event_allowlist=@('source_error')}
 Case 'canary position omitted' 'P0-EVID-PRIVACY-001' {param($c)$c.privacy_boundary.canary_positions=@($c.privacy_boundary.canary_positions|Where-Object{$_ -ne 'web_link'})}
 Case 'canary tolerated' 'P0-EVID-PRIVACY-001' {param($c)$c.privacy_boundary.prohibited_canary_count=1}
 Case 'raw ID plaintext' 'P0-EVID-PRIVACY-001' {param($c)$c.privacy_boundary.raw_identifier_storage='plaintext allowed'}
 Case 'anchor input persisted' 'P0-EVID-PRIVACY-001' {param($c)$c.privacy_boundary.transient_values='persist canonical blocks'}

 Case 'Graph call claim' 'P0-EVID-CLAIMS-001' {param($c)$c.claims.graph_calls=@('synthetic')}
 Case 'Office call claim' 'P0-EVID-CLAIMS-001' {param($c)$c.claims.office_calls=@('synthetic')}
 Case 'record claim' 'P0-EVID-CLAIMS-001' {param($c)$c.claims.persistent_records_created=@('synthetic')}
 Case 'scenario completed claim' 'P0-EVID-CLAIMS-001' {param($c)$c.claims.acceptance_scenarios_completed=@('AS-10')}
 Case 'runtime Graph enabled' 'P0-EVID-CLAIMS-001' {param($c)$c.runtime_boundary.graph_transport=$true}
 Case 'runtime Office enabled' 'P0-EVID-CLAIMS-001' {param($c)$c.runtime_boundary.office_bridge=$true}
 Case 'configured account' 'P0-EVID-CLAIMS-001' {param($c)$c.runtime_boundary.configured_accounts=@('synthetic')}
 Case 'network origin' 'P0-EVID-CLAIMS-001' {param($c)$c.runtime_boundary.network_origins=@('https://synthetic.invalid')}
 Case 'runtime file allowlist widened' 'P0-EVID-CLAIMS-001' {param($c)$c.inactive_runtime_inventory.allowed_files_in_order+=@('outlook-addin/src/synthetic.ts')}
 Case 'TypeScript import allowlist widened' 'P0-EVID-CLAIMS-001' {param($c)$c.inactive_runtime_inventory.allowed_typescript_import_lines_in_order+=@('import * as https from "node:https";')}
 Case 'Rust import allowlist widened' 'P0-EVID-CLAIMS-001' {param($c)$c.inactive_runtime_inventory.allowed_rust_use_lines_in_order+=@('use std::net::TcpStream;')}
 Case 'npm script allowlist changed' 'P0-EVID-CLAIMS-001' {param($c)$c.inactive_runtime_inventory.allowed_npm_scripts.lint='node synthetic-network.js'}
 Case 'npm dependency allowlist changed' 'P0-EVID-CLAIMS-001' {param($c)$c.inactive_runtime_inventory.allowed_dev_dependencies|Add-Member undici '0.0.0-synthetic'}
 Case 'package section prohibition removed' 'P0-EVID-CLAIMS-001' {param($c)$c.inactive_runtime_inventory.forbidden_package_sections=@($c.inactive_runtime_inventory.forbidden_package_sections|Where-Object{$_ -ne 'dependencies'})}
 Case 'execution surface fingerprint changed' 'P0-EVID-CLAIMS-001' {param($c)$c.inactive_runtime_inventory.execution_surface_fingerprint_sha256='synthetic'}
 Case 'format check permits implicit config' 'P0-EVID-CLAIMS-001' {param($c)$c.inactive_runtime_inventory.allowed_npm_scripts.'format:check'=$c.inactive_runtime_inventory.allowed_npm_scripts.'format:check'-replace' --no-config --no-editorconfig',''}
 Case 'quote vector changed' 'P0-EVID-ANCHOR-001' {param($c)($c.canonical_projection_contract.conformance_vectors|Where-Object id -eq 'CANON-HTML-ENTITY-BLOCKQUOTE-001').quote_blocks=@('CHANGED')}

 Case 'deferred calendar owner drift' 'P0-EVID-CROSS-CONTRACT-001' {param($c)$c.separate_decisions.calendar_event_response_values='ADR-006'}
 Case 'deferred artifact owner drift' 'P0-EVID-CROSS-CONTRACT-001' {param($c)$c.separate_decisions.user_authored_artifact_locator_and_field_values='ADR-006'}
 Case 'deferred bridge owner drift' 'P0-EVID-CROSS-CONTRACT-001' {param($c)$c.separate_decisions.add_in_bridge_pairing_and_client_support='ADR-006'}
 Case 'conflict omitted' 'P0-EVID-CROSS-CONTRACT-001' {param($c)$c.documented_conflicts_and_unresolved_tests=@($c.documented_conflicts_and_unresolved_tests|Select-Object -Skip 1)}
 DocCase 'governance ADR planned' 'P0-EVID-CROSS-CONTRACT-001' 'contracts/governance/capabilities.json' '-GovernancePath' {param($t)$t-replace'"id": "ADR-006", "status": "accepted"','"id": "ADR-006", "status": "planned"'}
 DocCase 'governance gate passed' 'P0-EVID-CROSS-CONTRACT-001' 'contracts/governance/capabilities.json' '-GovernancePath' {param($t)$t-replace'"id": "G-MAIL",\s+"status": "unrun"','"id": "G-MAIL", "status": "passed"'}
 DocCase 'permission Mail.Read drift' 'P0-EVID-CROSS-CONTRACT-001' 'contracts/identity/permission-boundary.json' '-PermissionPath' {param($t)$t-replace'"permission": "Mail.Read"','"permission": "User.Read"'}
 DocCase 'privacy source field removed' 'P0-EVID-CROSS-CONTRACT-001' 'contracts/privacy/persistence-boundary.json' '-PrivacyPath' {param($t)$t-replace'"source_ref_id", "account_ref"','"source_ref_id"'}
 DocCase 'sync ADR-006 status drift' 'P0-EVID-CROSS-CONTRACT-001' 'contracts/synchronization/mail-sync-boundary.json' '-SynchronizationPath' {param($t)$t-replace'shape_only_unimplemented_under_ADR-006_pending_G-MAIL','shape_only_unimplemented_pending_G-MAIL_and_ADR-006'}
 DocCase 'protected digest layout drift' 'P0-EVID-CROSS-CONTRACT-001' 'contracts/persistence/protected-state-boundary.json' '-ProtectedStatePath' {param($t)$t-replace'ADR-006 canonical_anchor_contract content_input and framing','unavailable_pending_owner_ADR'}
 DocCase 'protected mailbox purpose removed' 'P0-EVID-CROSS-CONTRACT-001' 'contracts/persistence/protected-state-boundary.json' '-ProtectedStatePath' {param($t)$t-replace'openloops-hmac-v1/email_evidence_ref.message_locator/mailbox_binding','synthetic-purpose'}
 DocCase 'support claim added' 'P0-EVID-CROSS-CONTRACT-001' 'contracts/support/support-matrix.json' '-SupportPath' {param($t)$t-replace'"supported_rows": \[\]','"supported_rows": ["synthetic"]'}
 DocCase 'build Graph runtime enabled' 'P0-EVID-CROSS-CONTRACT-001' 'contracts/build-skeleton/skeleton.json' '-BuildPath' {param($t)$t-replace'"graph_transport": false','"graph_transport": true'}
 DocCase 'ADR automatic reanchor wording removed' 'P0-EVID-CROSS-CONTRACT-001' 'docs/adr/ADR-006-evidence-identity-and-anchoring.md' '-AdrPath' {param($t)$t-replace'automatic reanchoring is prohibited','automatic replacement is available'}
 DocCase 'ADR exact runtime allowlist removed' 'P0-EVID-CROSS-CONTRACT-001' 'docs/adr/ADR-006-evidence-identity-and-anchoring.md' '-AdrPath' {param($t)$t-replace'exact allowlists','best-effort lists'}
 DocCase 'ADR deterministic DOM removed' 'P0-EVID-CROSS-CONTRACT-001' 'docs/adr/ADR-006-evidence-identity-and-anchoring.md' '-AdrPath' {param($t)$t-replace'DOM walker is byte-deterministic','DOM walker is implementation-defined'}
 DocCase 'threat copy rule weakened' 'P0-EVID-CROSS-CONTRACT-001' 'docs/threat-model/evidence-identity-and-navigation.md' '-ThreatPath' {param($t)$t-replace'Copy is always distinct','Copy may merge'}
 DocCase 'threat inline attachment control removed' 'P0-EVID-CROSS-CONTRACT-001' 'docs/threat-model/evidence-identity-and-navigation.md' '-ThreatPath' {param($t)$t-replace'Inline-only attachment name silently skipped','Inline attachments ignored'}
 DocCase 'trace check removed' 'P0-EVID-INVENTORY-001' 'docs/prd-traceability.md' '-TraceabilityPath' {param($t)$t-replace'(?m)^\| P0-EVID-GRAPH-001 \|.*\r?\n',''}
 WorkspaceCase 'TypeScript fetch transport introduced' {param($root)Set-Content -LiteralPath (Join-Path $root 'outlook-addin\src\synthetic-network.ts') -Value 'export const synthetic = () => fetch("https://synthetic.invalid");' -Encoding utf8NoBOM}
 WorkspaceCase 'Node HTTPS transport introduced' {param($root)Set-Content -LiteralPath (Join-Path $root 'outlook-addin\src\synthetic-node-https.ts') -Value 'import * as https from "node:https"; export const synthetic = () => https.get("https://synthetic.invalid");' -Encoding utf8NoBOM}
 WorkspaceCase 'Node HTTPS import added to approved file' {param($root)Add-Content -LiteralPath (Join-Path $root 'outlook-addin\src\skeleton.ts') -Value 'import * as https from "node:https";' -Encoding utf8NoBOM}
 WorkspaceCase 'JavaScript Office runtime introduced' {param($root)Set-Content -LiteralPath (Join-Path $root 'outlook-addin\src\synthetic-office.mjs') -Value 'export const synthetic = Office.context.mailbox;' -Encoding utf8NoBOM}
 WorkspaceCase 'Graph client dependency introduced' {param($root)$p=Get-Content -Raw (Join-Path $root 'package.json')|ConvertFrom-Json -Depth 100;if(-not$p.PSObject.Properties['dependencies']){$p|Add-Member dependencies ([pscustomobject]@{})};$p.dependencies|Add-Member '@microsoft/microsoft-graph-client' '0.0.0-synthetic';$p|ConvertTo-Json -Depth 100|Set-Content (Join-Path $root 'package.json') -Encoding utf8NoBOM}
 WorkspaceCase 'Undici dependency introduced' {param($root)$p=Get-Content -Raw (Join-Path $root 'package.json')|ConvertFrom-Json -Depth 100;$p|Add-Member dependencies ([pscustomobject]@{undici='0.0.0-synthetic'});$p|ConvertTo-Json -Depth 100|Set-Content (Join-Path $root 'package.json') -Encoding utf8NoBOM}
 WorkspaceCase 'Got dependency introduced' {param($root)$p=Get-Content -Raw (Join-Path $root 'package.json')|ConvertFrom-Json -Depth 100;$p|Add-Member dependencies ([pscustomobject]@{got='0.0.0-synthetic'});$p|ConvertTo-Json -Depth 100|Set-Content (Join-Path $root 'package.json') -Encoding utf8NoBOM}
 WorkspaceCase 'Arbitrary dependency introduced' {param($root)$p=Get-Content -Raw (Join-Path $root 'package.json')|ConvertFrom-Json -Depth 100;$p|Add-Member dependencies ([pscustomobject]@{'synthetic-package'='0.0.0-synthetic'});$p|ConvertTo-Json -Depth 100|Set-Content (Join-Path $root 'package.json') -Encoding utf8NoBOM}
 WorkspaceCase 'Network npm script introduced' {param($root)$p=Get-Content -Raw (Join-Path $root 'package.json')|ConvertFrom-Json -Depth 100;$p.scripts|Add-Member synthetic 'node -e "require(''node:https'').get(''https://synthetic.invalid'')"';$p|ConvertTo-Json -Depth 100|Set-Content (Join-Path $root 'package.json') -Encoding utf8NoBOM}
 WorkspaceCase 'Rust network import introduced' {param($root)Add-Content -LiteralPath (Join-Path $root 'crates\openloops-graph\src\lib.rs') -Value 'use std::net::TcpStream;' -Encoding utf8NoBOM}
 WorkspaceCase 'Generator dynamic network import introduced' {param($root)Add-Content -LiteralPath (Join-Path $root 'tools\generate-contracts.mjs') -Value 'await import("node:https");' -Encoding utf8NoBOM}
 WorkspaceCase 'Generator eval introduced' {param($root)Add-Content -LiteralPath (Join-Path $root 'tools\generate-contracts.mjs') -Value 'eval("synthetic");' -Encoding utf8NoBOM}
 WorkspaceCase 'ESLint config transport introduced' {param($root)Add-Content -LiteralPath (Join-Path $root 'eslint.config.mjs') -Value 'import * as https from "node:https";' -Encoding utf8NoBOM}
 WorkspaceCase 'Add-in test transport introduced' {param($root)Add-Content -LiteralPath (Join-Path $root 'outlook-addin\test\skeleton.test.ts') -Value 'import * as https from "node:https";' -Encoding utf8NoBOM}
 WorkspaceCase 'Lockfile-only dependency introduced' {param($root)$p=Get-Content -Raw (Join-Path $root 'package-lock.json')|ConvertFrom-Json -AsHashtable -Depth 100;$p.packages['node_modules/synthetic-arbitrary-dependency']=[ordered]@{version='0.0.0-synthetic'};$p|ConvertTo-Json -Depth 100|Set-Content (Join-Path $root 'package-lock.json') -Encoding utf8NoBOM}
 WorkspaceCase 'Cargo manifest-only dependency introduced' {param($root)$p=Join-Path $root 'crates\openloops-graph\Cargo.toml';$t=Get-Content -Raw $p;$t=$t-replace'openloops-domain = \{ path = "\.\./openloops-domain" \}',"openloops-domain = { path = `"../openloops-domain`" }`nserde = `"=1.0.229`"";Set-Content -LiteralPath $p -Value $t -Encoding utf8NoBOM -NoNewline}
 Write-Host "OpenLoops evidence identity negative suite passed ($caseNumber synthetic rejection cases)."
}finally{if([IO.Directory]::Exists($tempRoot)){[IO.Directory]::Delete($tempRoot,$true)}}
