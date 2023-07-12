window.SIDEBAR_ITEMS = {"constant":[["META_FEDERATION_NAME_KEY","Key under which the federation name can be sent to client in the `meta` part of the config"]],"enum":[["DkgError","Captures an error occurring in DKG"],["DkgMessage",""],["DkgPeerMsg","Things that a `distributed_gen` config can send between peers"],["SupportedDkgMessage","`enum` version of [`SupportedDkgMessage`]"]],"fn":[["load_from_file",""]],"mod":[["serde_binary_human_readable",""],["serde_commit","Handling the Group serialization with a wrapper"]],"struct":[["ClientConfig","Total client config"],["ClientConfigResponse","The API response for client config requests, signed by the Federation"],["ClientModuleConfig","Config for the client-side of a particular Federation module"],["ConfigGenModuleParams","Type erased `ModuleGenParams` used to generate the `ServerModuleConfig` during config gen"],["EmptyGenParams","Empty struct for if there are no params"],["FederationId","The federation id is a copy of the authentication threshold public key of the federation"],["JsonWithKind","[`serde_json::Value`] that must contain `kind: String` field"],["ModuleGenRegistry",""],["PeerUrl",""],["ServerModuleConfig","Config for the server-side of a particular Federation module"],["ServerModuleConsensusConfig",""]],"trait":[["DkgGroup","Defines a group (e.g. G1 or G2) that we can generate keys for"],["ISupportedDkgMessage","Supported (by Fedimint’s code) `DkgMessage<T>` types"],["ModuleGenParams",""],["SGroup",""],["TypedClientModuleConfig","Typed client side module config"],["TypedServerModuleConfig","Module (server side) config, typed"],["TypedServerModuleConsensusConfig","Consensus-critical part of a server side module config"]],"type":[["CommonModuleGenRegistry",""],["DkgResult","Result of running DKG"],["ServerModuleGenParamsRegistry","Registry that contains the config gen params for all modules"],["ServerModuleGenRegistry",""]]};