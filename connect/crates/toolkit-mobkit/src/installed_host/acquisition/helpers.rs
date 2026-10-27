//! Retained helper configurations, keyed only by their actual native member.
//! Route policy chooses one member; this inventory never chooses a route.
use super::*;

#[derive(Clone)]
pub(super) struct InstalledHelpers {
    default: (InstalledSetupSelection, CompiledNativeMember),
    additional: Vec<(InstalledSetupSelection, CompiledNativeMember)>,
}

impl InstalledHelpers {
    pub(super) fn new(
        default: InstalledSetupSelection,
        additional: Vec<InstalledSetupSelection>,
        native: &CompiledNativeHost,
    ) -> Result<Self, HostLifetimeError> {
        if additional.len() > 7 {
            return Err(HostLifetimeError::InvalidSelection);
        }
        let member = default.selected_member(native)?;
        let mut selected = Self { default: (default, member), additional: vec![] };
        for helper in additional {
            if selected.all().any(|(entry, _)| entry.member == helper.member) {
                return Err(HostLifetimeError::InvalidSelection);
            }
            let member = helper.selected_member(native)?;
            selected.additional.push((helper, member));
        }
        for member in &native.members {
            if let Some(helper) = member.setup_capability.as_ref().and_then(|value| value.helper.as_ref()) {
                selected.get(Some(helper))?;
            }
        }
        Ok(selected)
    }

    pub(super) fn get(&self, member: Option<&MobMemberBinding>)
        -> Result<(&InstalledSetupSelection, &CompiledNativeMember), HostLifetimeError>
    {
        match member {
            None => Ok((&self.default.0, &self.default.1)),
            Some(member) => self.all().find(|(helper, _)| &helper.member == member)
                .ok_or(HostLifetimeError::InvalidSelection),
        }
    }

    pub(super) fn all(&self) -> impl Iterator<Item = (&InstalledSetupSelection, &CompiledNativeMember)> {
        std::iter::once(&self.default).chain(self.additional.iter()).map(|(helper, member)| (helper, member))
    }
}
