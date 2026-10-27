interface DonorAction { run(context: { auth: Record<string, unknown> | string; propsValue: Record<string, unknown> }): Promise<unknown> }
export interface DonorResponse { status: number; headers: Record<string, string>; body: unknown }
export interface DonorBundle {
  activeCampaignCreateContact: DonorAction; freshserviceCreateTicket: DonorAction; giteaCreateIssue: DonorAction; datadogSubmitLog: DonorAction; gmailGetThread: DonorAction; slackGetThreadReplies: DonorAction; linearGetIssue: DonorAction; gcsSearchObjects: DonorAction;
  ghostGetPost:DonorAction; mondayGetItems:DonorAction; mailchimpGetReport:DonorAction; s3ListFiles:DonorAction;
  jiraGetIssue:DonorAction; dropboxGetFileMetadata:DonorAction; googleSheetsGetValues:DonorAction; excelGetTableColumns:DonorAction; teamsGetChannelMessage:DonorAction; quickbooksFindInvoice:DonorAction; xeroFindInvoice:DonorAction; algoliaBrowseRecords:DonorAction;
  freshdeskGetContact: DonorAction; googleFormsGetForm: DonorAction; googleSlidesGetPresentation: DonorAction; oneDriveGetItem: DonorAction; googleDocsGetDocument: DonorAction; googleDriveGetFile: DonorAction; contentfulGetRecord: DonorAction; outlookGetMessage: DonorAction; hubspotGetContact: DonorAction; supabaseGetUser: DonorAction; sendgridFindList: DonorAction; webflowGetItem: DonorAction;
  airtableGetRecord: DonorAction; togglFindProject: DonorAction; stripeGetInvoice: DonorAction; pipedriveGetActivity: DonorAction; wordpressGetPage: DonorAction; googleCalendarGetEvent: DonorAction; notionGetPage: DonorAction; intercomGetConversation: DonorAction;
  tallyGetForm: DonorAction; asanaGetTask: DonorAction; nocodbGetRecord: DonorAction;
  baserowGetRow: DonorAction; bitlyGetBitlinkDetails: DonorAction; clockifyFindRunningTimer: DonorAction;
  codaGetRow: DonorAction; pagerdutyGetIncident: DonorAction; postmarkGetDeliveryStats: DonorAction;
  typeformGetFormMessages: DonorAction;
  googleContactsSearch: DonorAction; figmaGetComments: DonorAction; acuityFindAppointments: DonorAction;
  confluenceGetPageContent: DonorAction; brevoFindContact: DonorAction; jotformGetSubmission: DonorAction;
  googleTasksGetTask: DonorAction; campaignMonitorFindSubscriber: DonorAction; oktaFindUserByEmail: DonorAction; helpScoutFindConversation: DonorAction;
  kitGetSubscriber: DonorAction; mailerLiteGetCampaign: DonorAction; calendlyGetScheduledEvent: DonorAction;
}
export function createDonorBundle(transport: (request: unknown) => Promise<DonorResponse>, context?:{mailchimpDataCenter?:string;invalidResponse:()=>never;s3Page?:(token:string|null)=>void}): DonorBundle;
