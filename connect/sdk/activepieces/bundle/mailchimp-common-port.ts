import { mailchimpCommon as original } from '../vendor/activepieces/packages/pieces/community/mailchimp/src/lib/common';
declare const __toolkitContext: {
    mailchimpDataCenter?: string;
};
export const mailchimpCommon = { ...original, async getMailChimpServerPrefix(_token: string) {
        if (!__toolkitContext.mailchimpDataCenter)
            throw new Error('Selected Mailchimp data center required');
        return __toolkitContext.mailchimpDataCenter;
    } };
