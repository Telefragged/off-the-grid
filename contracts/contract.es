{
  // decentralized grid order contract for token/ERG trading, with ERG accumulation

  // script of order owner
  val ownerScript = SELF.R4[SigmaProp].get

  // token price for buy and sell respectively, in nanoErg per token
  val tokenPrices = SELF.R5[(Long, Long)].get

  //BUY means we are buying tokens with ERGs, SELL means we sell tokens for ERGs
  val order = SELF.R6[(Coll[Byte], Long)].get

  val tokenId = order._1

  val orderSize = order._2

  //our order side, TRUE == BUY, FALSE == SELL
  val side = SELF.tokens.size == 0

  val tokenPrice = if (side) {
    tokenPrices._1
  } else {
    tokenPrices._2
  }

  val selfIndex = INPUTS.indexOf(SELF, 0)

  val recreatedBox = OUTPUTS(selfIndex)

  // check conditions not related to trading here
  val orderRecreated = (
      recreatedBox.propositionBytes == SELF.propositionBytes &&
      recreatedBox.R4[SigmaProp].get == SELF.R4[SigmaProp].get &&
      recreatedBox.R5[(Long, Long)].get == SELF.R5[(Long, Long)].get &&
      recreatedBox.R6[(Coll[Byte], Long)].get == SELF.R6[(Coll[Byte], Long)].get
  )

  val metadataRecreated = if(SELF.R7[Coll[Byte]].isDefined) {
      // Enforce recreation of additional data which can be used for order grouping and tracking profits
      recreatedBox.R7[Coll[Byte]].get == SELF.R7[Coll[Byte]].get
  } else {
      true
  }

  val nanoErgsDifference = if(side) {
    // we are buying token - should be more ERG in our order box than in child
    SELF.value - recreatedBox.value
  } else {
    // we are selling token - so should be more in child order than ours
    recreatedBox.value - SELF.value
  }

  val tokensCheck = if(side) {
    // check ID and amount of token we're buying
    recreatedBox.tokens.size == 1 &&
    recreatedBox.tokens(0)._1 == tokenId &&
    recreatedBox.tokens(0)._2 == orderSize
  } else {
    // if we're selling tokens, we sell all of them
    recreatedBox.tokens.size == 0
  }

  val exchangeOK = if(side) {
    nanoErgsDifference <= orderSize * tokenPrice
  } else {
    nanoErgsDifference >= orderSize * tokenPrice
  }

  val totalFee = OUTPUTS.fold(0L, {
    (fee:Long, b:Box) =>
      if (b.propositionBytes == FeeProposition) fee + b.value else fee
  })

  val feeOk = totalFee == MaxFee

  sigmaProp(
    ownerScript ||
    (
        orderRecreated &&
        metadataRecreated &&
        exchangeOK &&
        (nanoErgsDifference > 0) &&
        tokensCheck &&
        feeOk
    )
  )
}
