import {nextProfiles} from './next-profiles.mjs';
import {oauthProfiles} from './oauth-profiles.mjs';
import {FIXTURES} from './content-profiles.mjs';
import {serviceProfiles} from './service-profiles.mjs';
import {businessProfiles} from './business-profiles.mjs';
import {platformProfiles} from './platform-profiles.mjs';
import {contentExpansionProfiles} from './content-expansion-profiles.mjs';
import {readExpansionProfiles} from './read-expansion-profiles.mjs';
import {sdkProfiles} from './sdk-expansion-profiles.mjs';
import {cases} from '../effects-fixtures.mjs';

export const configurations = [
  {profile:'tally.get-form', token_env:'SELECTED_TOKEN'},
  {profile:'asana.get-task', token_env:'SELECTED_TOKEN'},
  {profile:'nocodb.get-record', token_env:'SELECTED_TOKEN', origin:'https://selected.invalid', version:4},
  ...[...nextProfiles, ...oauthProfiles, ...FIXTURES, ...serviceProfiles, ...businessProfiles,
      ...platformProfiles, ...contentExpansionProfiles, ...readExpansionProfiles, ...sdkProfiles]
    .map(item=>({profile:item.profile, token_env:'SELECTED_TOKEN', ...(item.config??item.mcp??{})})),
  ...cases.map(item=>({profile:item.profile, token_env:'SELECTED_TOKEN',
    ...Object.fromEntries(['origin','domain','owner','repository','site','bucket'].filter(key=>key in item.auth).map(key=>[key,item.auth[key]]))})),
];

